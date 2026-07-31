//! Shell-command classification — a port of agentpm's
//! `ClassifyShellCommand` (tooltree/types.go). Produces the `tool_type:`
//! vocabulary plus the privileged/remote/read-only qualifiers behind
//! `tool_privilege:`, `tool_location:`, and `tool_mutability:`.
//!
//! Match order is load-bearing: the first matching rule wins, mirroring
//! the reference switch statement.

use crate::shell::{expand_wrapper, fields, split_segments};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ShellClassification {
    /// Slugged subtype (`git-operation`, `tests`, …) or None.
    pub subtype: Option<&'static str>,
    /// Any segment's raw first executable is sudo.
    pub privileged: bool,
    /// Any segment's raw first executable is ssh/scp/rsync.
    pub remote: bool,
    /// The subtype (plus command words) implies no mutation. Only
    /// meaningful when `subtype` is Some.
    pub read_only: bool,
}

pub fn classify_shell_command(command: &str) -> ShellClassification {
    let value = command.trim();
    if value.is_empty() {
        return ShellClassification::default();
    }
    let lower = value.to_lowercase();
    let subtype = classify_subtype(&lower);
    ShellClassification {
        subtype,
        privileged: any_raw_executable(&lower, &["sudo"]),
        remote: any_raw_executable(&lower, &["ssh", "scp", "rsync"]),
        read_only: subtype.is_some_and(|s| read_only_command(&lower, s)),
    }
}

fn classify_subtype(lower: &str) -> Option<&'static str> {
    let exe = |names: &[&str]| contains_any_executable(lower, names);
    let word = |words: &[&str]| contains_any_word(lower, words);
    Some(match () {
        _ if exe(&["bd"]) => "issue-tracking",
        _ if exe(&["gh"])
            && word(&["create", "edit", "merge", "close", "reopen", "comment", "review"]) =>
        {
            "git-operation"
        }
        _ if exe(&["gh"]) && word(&["status", "list", "view", "diff", "checks"]) => "git-inspection",
        _ if exe(&["git"])
            && word(&["add", "commit", "push", "pull", "rebase", "merge", "restore"]) =>
        {
            "git-operation"
        }
        _ if exe(&["git"])
            && word(&["status", "diff", "log", "show", "remote", "rev-parse", "ls-remote", "merge-base"]) =>
        {
            "git-inspection"
        }
        _ if package_management(lower) => "package-management",
        _ if exe(&["rsync", "scp", "sftp"]) => "file-transfer",
        _ if exe(&["sqlite3", "psql", "mysql"])
            && (word(&["select", "show", "describe", "schema", "tables", "pragma"])
                || lower.contains(".schema")) =>
        {
            "database-inspection"
        }
        _ if exe(&["sqlite3", "psql", "mysql"])
            && word(&["insert", "update", "delete", "create", "drop", "alter", "vacuum", "reindex"]) =>
        {
            "database-operation"
        }
        _ if exe(&["pytest"])
            || (exe(&["go", "cargo", "uv"]) && lower.contains(" test"))
            || (exe(&["npm", "pnpm", "yarn"]) && word(&["test"])) =>
        {
            "tests"
        }
        _ if (exe(&["go"]) && word(&["build", "install"])) || exe(&["make", "cmake"]) => "build",
        _ if exe(&["gofmt", "prettier", "eslint", "rustfmt"]) => "formatting",
        _ if exe(&["install", "ln"]) && lower.contains("/usr/local/bin/") => "deployment",
        _ if exe(&["ss", "netstat", "lsof"]) => "network-inspection",
        _ if exe(&["kill", "pkill"]) => "process-control",
        _ if exe(&["pgrep", "ps"]) => "process-inspection",
        _ if exe(&["tmux"]) => "terminal-session",
        _ if exe(&["date"]) => "time-lookup",
        _ if exe(&["pwd"]) => "directory-inspection",
        _ if inline_python(lower) => "inline-python-code",
        _ if exe(&["python", "python3"]) && lower.contains(" -m ") => "python-module",
        _ if heredoc_python_script(lower) => "python-script",
        _ if lower.contains("node -e") => "inline-node-code",
        _ if lower.contains("perl -e") || lower.contains("perl -0") || lower.contains("perl -p") => {
            "inline-perl-code"
        }
        _ if lower.contains("ruby -e") => "inline-ruby-code",
        _ if lower.contains("--version") || word(&["version"]) => "tool-version",
        _ if word(&["npm", "pnpm", "yarn"]) && word(&["build", "lint", "test", "typecheck"]) => {
            "project-script"
        }
        _ if exe(&["rg", "grep", "find"]) => "search",
        _ if exe(&["curl", "wget"]) => "http-request",
        _ if exe(&["systemctl", "journalctl", "service", "pm2"])
            || word(&["systemctl", "journalctl", "service"]) =>
        {
            "service-operation"
        }
        _ if exe(&["sleep"]) || (exe(&["tail"]) && lower.contains("/tmp/claude-")) => {
            "task-monitoring"
        }
        _ if exe(&["rm"]) => "cleanup",
        _ if exe(&["mkdir", "touch"]) => "filesystem-update",
        _ if exe(&["ls", "cat", "head", "tail", "wc"]) => "file-inspection",
        _ if exe(&["printf"]) => "generated-file",
        _ if numbered_file_read_pipeline(lower) => "file-inspection",
        _ if sed_file_read(lower) => "file-inspection",
        _ if exe(&["sed", "awk", "perl"]) => "text-processing",
        _ if lower.contains(".py") && exe(&["python", "python3"]) => "python-script",
        _ => return None,
    })
}

fn read_only_command(lower: &str, subtype: &str) -> bool {
    match subtype {
        "git-inspection" | "search" | "file-inspection" | "text-processing"
        | "process-inspection" | "network-inspection" | "tool-version" | "time-lookup"
        | "directory-inspection" | "terminal-session" | "database-inspection" => true,
        "service-operation" => {
            contains_any_word(lower, &["status", "list", "show", "logs", "describe"])
                || contains_any_executable(lower, &["journalctl"])
        }
        "http-request" => {
            !contains_any_word(lower, &["post", "put", "patch", "delete"])
                && !lower.contains(" -d ")
                && !lower.contains(" --data")
        }
        "issue-tracking" => contains_any_word(lower, &["show", "ready", "list", "status"]),
        _ => false,
    }
}

fn package_management(lower: &str) -> bool {
    let install_words = ["install", "uninstall", "download", "wheel", "freeze"];
    if contains_any_executable(lower, &["pip"]) && contains_any_word(lower, &install_words) {
        return true;
    }
    if contains_any_executable(lower, &["python", "python3"])
        && lower.contains(" -m pip ")
        && contains_any_word(lower, &install_words)
    {
        return true;
    }
    contains_any_executable(lower, &["npm", "pnpm", "yarn"])
        && contains_any_word(lower, &["install", "add", "remove", "update", "ci"])
}

fn inline_python(lower: &str) -> bool {
    for command in classification_commands(lower) {
        let f = fields(&command);
        for (i, field) in f.iter().enumerate() {
            let name = field.trim_start_matches(['"', '\'', '(']);
            let name = name.rsplit('/').next().unwrap_or(name);
            if !is_python_executable(name) || i + 1 >= f.len() {
                continue;
            }
            let next = &f[i + 1];
            if next == "-c" || next.starts_with("<<") {
                return true;
            }
            if next == "-" && i + 2 < f.len() && f[i + 2].starts_with("<<") {
                return true;
            }
        }
    }
    false
}

fn heredoc_python_script(lower: &str) -> bool {
    for segment in expanded_segments(lower) {
        let command = &segment.text;
        if !command.contains("<<") || !command.contains(".py") {
            continue;
        }
        if first_executable_name(command) == "cat" && command.contains('>') {
            return true;
        }
    }
    false
}

fn numbered_file_read_pipeline(lower: &str) -> bool {
    let segments = expanded_segments(lower);
    for (index, segment) in segments.iter().enumerate() {
        if first_executable_name(&segment.text) != "nl" || !nl_has_file_operand(&segment.text) {
            continue;
        }
        for next in &segments[index + 1..] {
            if next.operator_before != "|" {
                break;
            }
            if first_executable_name(&next.text) == "sed" && sed_print_only(&next.text) {
                return true;
            }
        }
    }
    false
}

fn nl_has_file_operand(command: &str) -> bool {
    let f = fields(command);
    let mut seen_nl = false;
    let mut index = 0;
    while index < f.len() {
        let field = &f[index];
        if !seen_nl {
            if executable_name(field) == "nl" {
                seen_nl = true;
            }
            index += 1;
            continue;
        }
        if field == "--" {
            return index + 1 < f.len();
        }
        if field.starts_with('-') {
            if nl_option_takes_value(field) && index + 1 < f.len() {
                index += 1;
            }
            index += 1;
            continue;
        }
        return true;
    }
    false
}

fn nl_option_takes_value(option: &str) -> bool {
    option.len() <= 2
        && matches!(option, "-b" | "-d" | "-f" | "-h" | "-i" | "-l" | "-n" | "-s" | "-v" | "-w")
}

fn sed_print_only(command: &str) -> bool {
    let f = fields(command);
    let Some(sed_index) = f.iter().position(|field| executable_name(field) == "sed") else {
        return false;
    };
    let mut has_print_only = false;
    for field in &f[sed_index + 1..] {
        if field == "-i" || field.starts_with("-i") || field == "--in-place" || field.starts_with("--in-place=") {
            return false;
        }
        if field == "-n" || (field.starts_with('-') && field.contains('n')) {
            has_print_only = true;
            continue;
        }
        if field.starts_with('-') {
            continue;
        }
        if sed_print_expression(field) {
            has_print_only = true;
            continue;
        }
        return false;
    }
    has_print_only
}

fn sed_file_read(lower: &str) -> bool {
    for command in classification_commands(lower) {
        if first_executable_name(&command) != "sed" {
            continue;
        }
        let f = fields(&command);
        let Some(sed_index) = f.iter().position(|field| executable_name(field) == "sed") else {
            continue;
        };
        let mut has_print_only = false;
        let mut has_file_operand = false;
        let mut skip = false;
        for (offset, field) in f[sed_index + 1..].iter().enumerate() {
            if field == "--" {
                has_file_operand = f[sed_index + 1 + offset + 1..]
                    .iter()
                    .any(|operand| !operand.starts_with('-'));
                break;
            }
            if field == "-i" || field.starts_with("-i") || field == "--in-place" || field.starts_with("--in-place=") {
                skip = true;
                break;
            }
            if field == "-n" || (field.starts_with('-') && field.contains('n')) {
                has_print_only = true;
                continue;
            }
            if field.starts_with('-') {
                continue;
            }
            if sed_print_expression(field) {
                has_print_only = true;
                continue;
            }
            has_file_operand = true;
        }
        if !skip && has_print_only && has_file_operand {
            return true;
        }
    }
    false
}

fn sed_print_expression(value: &str) -> bool {
    let value = value.trim();
    !value.is_empty() && !value.contains(['s', 'c', 'i', 'a', 'y', 'w', 'q', 'r', '{', '}', '=']) && value.ends_with('p')
}

fn is_python_executable(name: &str) -> bool {
    if name == "python" {
        return true;
    }
    let Some(rest) = name.strip_prefix("python3") else {
        return false;
    };
    if rest.is_empty() {
        return true;
    }
    let Some(version) = rest.strip_prefix('.') else {
        return false;
    };
    !version.is_empty()
        && version
            .split('.')
            .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
}

/// Every command reachable from the value: each segment (wrappers
/// expanded), plus the payloads of `ssh host '…'` invocations.
fn classification_commands(value: &str) -> Vec<String> {
    expanded_segments(value).into_iter().map(|s| s.text).collect()
}

fn expanded_segments(value: &str) -> Vec<crate::shell::Segment> {
    let mut out = Vec::new();
    for segment in split_segments(value) {
        let text = expand_wrapper(&segment.text).unwrap_or(segment.text);
        if let Some(payload) = ssh_remote_payload(&text) {
            out.push(crate::shell::Segment { text: text.clone(), operator_before: segment.operator_before });
            for inner in split_segments(&payload) {
                let inner_text = expand_wrapper(&inner.text).unwrap_or(inner.text);
                out.push(crate::shell::Segment { text: inner_text, operator_before: inner.operator_before });
            }
        } else {
            out.push(crate::shell::Segment { text, operator_before: segment.operator_before });
        }
    }
    out
}

fn contains_any_executable(value: &str, names: &[&str]) -> bool {
    classification_commands(value).iter().any(|command| {
        let mut name = first_executable_name(command);
        if is_python_executable(&name) {
            name = "python3".to_string();
        }
        names.contains(&name.as_str())
    })
}

fn any_raw_executable(value: &str, names: &[&str]) -> bool {
    expanded_segments(value)
        .iter()
        .any(|segment| names.contains(&raw_first_executable_name(&segment.text).as_str()))
}

fn contains_any_word(value: &str, words: &[&str]) -> bool {
    value
        .split(|c: char| !(c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-'))
        .any(|field| !field.is_empty() && words.contains(&field))
}

/// The effective first executable: skips VAR=val assignments and
/// sudo/env/command wrappers, recurses into `bash -c` and ssh payloads.
pub fn first_executable_name(command: &str) -> String {
    let f = fields(command);
    let mut index = 0;
    while index < f.len() {
        let field = &f[index];
        if field.contains('=') && !field.contains('/') {
            index += 1;
            continue;
        }
        let name = executable_name(field);
        match name.as_str() {
            "sudo" | "env" | "command" => {
                index += 1;
                while index < f.len() && f[index].starts_with('-') {
                    index += 1;
                }
            }
            "bash" | "sh" | "zsh" => {
                if index + 2 < f.len() && (f[index + 1] == "-c" || f[index + 1] == "-lc") {
                    return first_executable_name(&f[index + 2..].join(" "));
                }
                return name;
            }
            "ssh" => {
                if let Some(payload) = ssh_remote_payload(&f[index..].join(" ")) {
                    return first_executable_name(&payload);
                }
                return name;
            }
            _ => return name,
        }
    }
    String::new()
}

/// The literal first executable (no wrapper skipping) — sudo/ssh detection.
fn raw_first_executable_name(command: &str) -> String {
    for field in fields(command) {
        if field.contains('=') && !field.contains('/') {
            continue;
        }
        return executable_name(&field);
    }
    String::new()
}

fn executable_name(field: &str) -> String {
    let trim_set: &[char] = &['"', '\'', '`', '(', ')', '[', ']', '{', '}'];
    let mut name = field.trim_matches(|c| trim_set.contains(&c)).to_string();
    while let Some((prefix, rest)) = name.split_once('=') {
        if prefix.is_empty() || prefix.contains('/') {
            break;
        }
        name = rest.to_string();
    }
    if let Some(base) = name.rsplit('/').next() {
        name = base.to_string();
    }
    name.trim_matches(|c: char| trim_set.contains(&c) || matches!(c, '.' | ',' | ':')).to_string()
}

fn ssh_option_takes_value(option: &str) -> bool {
    !option.contains('=')
        && matches!(
            option,
            "-b" | "-c" | "-D" | "-E" | "-F" | "-I" | "-i" | "-J" | "-L" | "-l" | "-m" | "-O"
                | "-o" | "-p" | "-Q" | "-R" | "-S" | "-W" | "-w"
        )
}

/// The remote command of `ssh [flags] host cmd…`, if any.
fn ssh_remote_payload(command: &str) -> Option<String> {
    let f = fields(command);
    let ssh_index = f.iter().position(|field| executable_name(field) == "ssh")?;
    let mut index = ssh_index + 1;
    while index < f.len() {
        let field = &f[index];
        if field == "--" {
            index += 1;
            break;
        }
        if !field.starts_with('-') {
            break;
        }
        let takes_value = ssh_option_takes_value(field);
        index += 1;
        if takes_value && index < f.len() {
            index += 1;
        }
    }
    // f[index] is the host; the payload is everything after it.
    if index < f.len() {
        index += 1;
    }
    if index < f.len() {
        Some(f[index..].join(" "))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn subtype(cmd: &str) -> Option<&'static str> {
        classify_shell_command(cmd).subtype
    }

    #[test]
    fn vocabulary_by_rule() {
        assert_eq!(subtype("bd show rogrep-12"), Some("issue-tracking"));
        assert_eq!(subtype("gh pr merge 48 --squash"), Some("git-operation"));
        assert_eq!(subtype("gh pr view 48"), Some("git-inspection"));
        assert_eq!(subtype("git commit -m 'x'"), Some("git-operation"));
        assert_eq!(subtype("git log --oneline"), Some("git-inspection"));
        assert_eq!(subtype("pip install requests"), Some("package-management"));
        assert_eq!(subtype("npm ci"), Some("package-management"));
        assert_eq!(subtype("scp a b:/tmp"), Some("file-transfer"));
        assert_eq!(subtype("sqlite3 db.sqlite 'select 1'"), Some("database-inspection"));
        assert_eq!(subtype("sqlite3 db.sqlite 'vacuum'"), Some("database-operation"));
        assert_eq!(subtype("cargo test --workspace"), Some("tests"));
        assert_eq!(subtype("pytest -x"), Some("tests"));
        assert_eq!(subtype("go build ./..."), Some("build"));
        assert_eq!(subtype("make -j8"), Some("build"));
        assert_eq!(subtype("rustfmt src/lib.rs"), Some("formatting"));
        assert_eq!(subtype("ln -sf /opt/x /usr/local/bin/x"), Some("deployment"));
        assert_eq!(subtype("lsof -i :8080"), Some("network-inspection"));
        assert_eq!(subtype("pkill -f server"), Some("process-control"));
        assert_eq!(subtype("ps aux"), Some("process-inspection"));
        assert_eq!(subtype("tmux ls"), Some("terminal-session"));
        assert_eq!(subtype("date +%s"), Some("time-lookup"));
        assert_eq!(subtype("pwd"), Some("directory-inspection"));
        assert_eq!(subtype("python3 -c 'print(1)'"), Some("inline-python-code"));
        assert_eq!(subtype("python3 -m json.tool f"), Some("python-module"));
        assert_eq!(subtype("cat > x.py <<'EOF'\nprint(1)\nEOF"), Some("python-script"));
        assert_eq!(subtype("node -e 'console.log(1)'"), Some("inline-node-code"));
        assert_eq!(subtype("perl -e 'print 1'"), Some("inline-perl-code"));
        assert_eq!(subtype("ruby -e 'puts 1'"), Some("inline-ruby-code"));
        assert_eq!(subtype("rustc --version"), Some("tool-version"));
        assert_eq!(subtype("rg -n TODO src/"), Some("search"));
        assert_eq!(subtype("curl -s https://x.dev"), Some("http-request"));
        assert_eq!(subtype("systemctl status agentpm"), Some("service-operation"));
        assert_eq!(subtype("sleep 30"), Some("task-monitoring"));
        assert_eq!(subtype("rm -rf target/"), Some("cleanup"));
        assert_eq!(subtype("mkdir -p a/b"), Some("filesystem-update"));
        assert_eq!(subtype("cat notes.txt"), Some("file-inspection"));
        assert_eq!(subtype("printf 'x\\n' > f"), Some("generated-file"));
        assert_eq!(subtype("nl -ba src/main.rs | sed -n '1,40p'"), Some("file-inspection"));
        assert_eq!(subtype("sed -n '10,20p' src/main.rs"), Some("file-inspection"));
        assert_eq!(subtype("awk '{print $1}' f"), Some("text-processing"));
        assert_eq!(subtype("python3 scripts/gen.py"), Some("python-script"));
        assert_eq!(subtype("some-custom-binary --flag"), None);
        assert_eq!(subtype(""), None);
    }

    #[test]
    fn first_match_wins() {
        // git push is git-operation even though "test" appears as a word.
        assert_eq!(subtype("git push origin test"), Some("git-operation"));
        // sed with -i falls through file-inspection to text-processing.
        assert_eq!(subtype("sed -i 's/a/b/' f.txt"), Some("text-processing"));
    }

    #[test]
    fn qualifiers() {
        let c = classify_shell_command("sudo rm -rf /var/tmp/x");
        assert_eq!(c.subtype, Some("cleanup"));
        assert!(c.privileged);
        assert!(!c.read_only);

        let c = classify_shell_command("ssh prod 'git pull origin main'");
        assert_eq!(c.subtype, Some("git-operation"));
        assert!(c.remote);
        assert!(!c.privileged);

        let c = classify_shell_command("git log --oneline | head -3");
        assert!(c.read_only);

        let c = classify_shell_command("curl -X POST -d x=1 https://api.dev");
        assert_eq!(c.subtype, Some("http-request"));
        assert!(!c.read_only);
        let c = classify_shell_command("curl -s https://api.dev/health");
        assert!(c.read_only);

        let c = classify_shell_command("systemctl restart agentpm");
        assert!(!c.read_only);
        let c = classify_shell_command("journalctl -u agentpm -n 50");
        assert!(c.read_only);
    }

    #[test]
    fn wrappers_and_payloads() {
        assert_eq!(subtype("bash -c 'cargo test'"), Some("tests"));
        assert_eq!(subtype("sudo systemctl restart x"), Some("service-operation"));
        assert_eq!(subtype("ssh -p 2222 host 'git status'"), Some("git-inspection"));
        assert_eq!(first_executable_name("FOO=1 sudo -E git push"), "git");
        assert_eq!(first_executable_name("bash -lc 'rg pattern'"), "rg");
    }
}
