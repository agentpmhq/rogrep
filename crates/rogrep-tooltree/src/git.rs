//! git / gh command classification and output mining.
//!
//! Produces the structured facet tokens the trajectory command depends on:
//! `git_cmd:<verb>`, `git_pr:<action>`, `git_pr_num:N`, `git_commit:<sha7>`,
//! `git_branch:<name>`, `git_remote:<name>`.
//!
//! The load-bearing subtlety (from agentpm): the PR number for
//! `gh pr create` exists only in the command OUTPUT (the GitHub URL), never
//! in the command itself — `mine_output` pairs it back.

use crate::shell::{base_name, fields};

pub const SHORT_SHA_LEN: usize = 7;

const GIT_VERBS: &[&str] = &[
    "add", "am", "apply", "bisect", "blame", "branch", "checkout", "cherry-pick", "clean",
    "clone", "commit", "diff", "fetch", "grep", "init", "log", "merge", "mv", "pull", "push",
    "rebase", "remote", "reset", "restore", "revert", "rm", "show", "stash", "status", "switch",
    "tag", "worktree",
];

const READ_ONLY_VERBS: &[&str] = &[
    "status", "diff", "log", "show", "blame", "grep", "describe", "rev-parse", "ls-files",
    "ls-remote", "shortlog", "reflog",
];

const PR_ACTIONS: &[&str] = &[
    "create", "merge", "close", "reopen", "edit", "view", "list", "checkout", "comment", "diff",
    "ready", "review", "status", "checks",
];

const PR_MUTATING_ACTIONS: &[&str] = &["create", "merge", "close", "reopen", "edit", "ready", "review", "comment"];

pub fn short_sha(sha: &str) -> String {
    let s = sha.trim().to_lowercase();
    if s.len() > SHORT_SHA_LEN && s.chars().all(|c| c.is_ascii_hexdigit()) {
        s[..SHORT_SHA_LEN].to_string()
    } else {
        s
    }
}

fn looks_like_sha(s: &str) -> bool {
    s.len() >= 7 && s.len() <= 40 && s.chars().all(|c| c.is_ascii_hexdigit()) && s.chars().any(|c| c.is_ascii_digit())
}

fn looks_like_branch(s: &str) -> bool {
    !s.is_empty()
        && !s.starts_with('-')
        && s.len() < 120
        && s.chars().all(|c| c.is_alphanumeric() || "/_.-".contains(c))
        && !looks_like_sha(s)
}

/// Facets for one already-segmented shell command.
pub fn git_facets_for_segment(segment: &str) -> Vec<String> {
    let f = fields(segment);
    if f.is_empty() {
        return vec![];
    }
    let exe = base_name(&f[0]);
    match exe.as_str() {
        "git" => git_command_facets(&f[1..]),
        "gh" => gh_command_facets(&f[1..]),
        "ssh" => ssh_payload_facets(&f[1..]),
        _ => vec![],
    }
}

fn git_command_facets(args: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    // Skip global flags (-C dir, -c k=v, --git-dir=…).
    let mut idx = 0;
    while idx < args.len() {
        let a = &args[idx];
        if a == "-C" || a == "-c" {
            idx += 2;
            continue;
        }
        if a.starts_with('-') {
            idx += 1;
            continue;
        }
        break;
    }
    let Some(verb) = args.get(idx) else { return out };
    let verb = verb.to_lowercase();
    if !GIT_VERBS.contains(&verb.as_str()) {
        return out;
    }
    out.push(format!("git_cmd:{verb}"));
    let operands: Vec<&String> = args[idx + 1..].iter().filter(|a| !a.starts_with('-')).collect();
    match verb.as_str() {
        "push" | "pull" | "fetch" => {
            if let Some(remote) = operands.first() {
                out.push(format!("git_remote:{remote}"));
            }
            if let Some(refspec) = operands.get(1) {
                // `git push origin local:remote` → the remote branch.
                let branch = refspec.rsplit(':').next().unwrap_or(refspec);
                if looks_like_branch(branch) {
                    out.push(format!("git_branch:{branch}"));
                }
            }
        }
        "checkout" | "switch" | "merge" | "rebase" | "branch" => {
            for op in &operands {
                if looks_like_sha(op) {
                    out.push(format!("git_commit:{}", short_sha(op)));
                } else if looks_like_branch(op) {
                    out.push(format!("git_branch:{op}"));
                }
            }
        }
        "cherry-pick" | "revert" | "reset" | "show" => {
            for op in &operands {
                if looks_like_sha(op) {
                    out.push(format!("git_commit:{}", short_sha(op)));
                }
            }
        }
        "worktree" => {
            // `git worktree add path branch`.
            if operands.first().map(|s| s.as_str()) == Some("add") {
                if let Some(branch) = operands.get(2) {
                    if looks_like_branch(branch) {
                        out.push(format!("git_branch:{branch}"));
                    }
                }
            }
        }
        "tag" => {
            if let Some(name) = operands.first() {
                if looks_like_branch(name) {
                    out.push(format!("git_branch:{name}"));
                }
            }
        }
        _ => {}
    }
    out
}

fn gh_command_facets(args: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    if args.first().map(|s| s.as_str()) != Some("pr") {
        return out;
    }
    let Some(action) = args.get(1) else { return out };
    let action = action.to_lowercase();
    if !PR_ACTIONS.contains(&action.as_str()) {
        return out;
    }
    out.push(format!("git_pr:{action}"));
    // Positional PR number/url — EXCEPT for create/list, where a positional
    // number is never the PR being created.
    if !matches!(action.as_str(), "create" | "list") {
        for a in &args[2..] {
            if a.starts_with('-') {
                break;
            }
            if let Ok(n) = a.parse::<u64>() {
                out.push(format!("git_pr_num:{n}"));
                break;
            }
            if let Some(n) = pr_number_from_url(a) {
                out.push(format!("git_pr_num:{n}"));
                break;
            }
        }
    }
    // Branches from --head/--base.
    let mut iter = args.iter().peekable();
    while let Some(a) = iter.next() {
        for flag in ["--head", "--base", "-H", "-B"] {
            if a == flag {
                if let Some(v) = iter.peek() {
                    if looks_like_branch(v) {
                        out.push(format!("git_branch:{v}"));
                    }
                }
            } else if let Some(v) = a.strip_prefix(&format!("{flag}=")) {
                if looks_like_branch(v) {
                    out.push(format!("git_branch:{v}"));
                }
            }
        }
    }
    out
}

/// Recurse into `ssh host 'git …'` payloads.
fn ssh_payload_facets(args: &[String]) -> Vec<String> {
    // The payload is the first non-flag arg after the host.
    let mut non_flags = args.iter().filter(|a| !a.starts_with('-'));
    let _host = non_flags.next();
    let mut out = Vec::new();
    for payload in non_flags {
        for seg in crate::shell::split_segments(payload) {
            out.extend(git_facets_for_segment(&seg.text));
        }
    }
    out
}

pub fn pr_number_from_url(s: &str) -> Option<u64> {
    let idx = s.find("/pull/")?;
    let digits: String = s[idx + 6..].chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

/// Mine a tool OUTPUT for git evidence. `command_facets` are the facets of
/// the paired command, used to label `gh pr create` output URLs correctly.
pub fn mine_output(output: &str, command_facets: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    let head: String = output.chars().take(4000).collect();
    // PR URLs.
    let mut search = head.as_str();
    while let Some(idx) = search.find("github.com/") {
        let rest = &search[idx..];
        if let Some(n) = pr_number_from_url(rest) {
            out.push(format!("git_pr_num:{n}"));
            if command_facets.iter().any(|f| f == "git_pr:create") {
                // The create action's number lives only here.
                out.push(format!("git_pr:create-num:{n}"));
            }
        }
        search = &search[idx + 11..];
    }
    // `[branch abc1234] message` commit confirmations.
    let mut scan = head.as_str();
    while let Some(open) = scan.find('[') {
        let rest = &scan[open + 1..];
        if let Some(close) = rest.find(']') {
            let inside = &rest[..close];
            if let Some(sha) = inside.split_whitespace().last() {
                if looks_like_sha(sha)
                    && command_facets.iter().any(|f| f == "git_cmd:commit")
                {
                    out.push(format!("git_commit:{}", short_sha(sha)));
                }
            }
            scan = &rest[close..];
        } else {
            break;
        }
    }
    // `abc1234..def5678` push ranges.
    for token in head.split_whitespace() {
        if let Some((a, b)) = token.split_once("..") {
            if looks_like_sha(a) && looks_like_sha(b) {
                out.push(format!("git_commit:{}", short_sha(a)));
                out.push(format!("git_commit:{}", short_sha(b)));
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Is this git verb / pr action mutating? Drives `--mutating-only`.
pub fn is_mutating_facet(facet: &str) -> Option<bool> {
    if let Some(verb) = facet.strip_prefix("git_cmd:") {
        return Some(!READ_ONLY_VERBS.contains(&verb));
    }
    if let Some(action) = facet.strip_prefix("git_pr:") {
        return Some(PR_MUTATING_ACTIONS.contains(&action));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn git_commit_and_push() {
        assert_eq!(git_facets_for_segment("git commit -m 'fix'"), vec!["git_cmd:commit"]);
        assert_eq!(
            git_facets_for_segment("git push origin feature/x"),
            vec!["git_cmd:push", "git_remote:origin", "git_branch:feature/x"]
        );
        assert_eq!(
            git_facets_for_segment("git push origin local:remote-branch"),
            vec!["git_cmd:push", "git_remote:origin", "git_branch:remote-branch"]
        );
    }

    #[test]
    fn gh_pr_create_number_not_from_command() {
        let f = git_facets_for_segment("gh pr create --title 'x' --head feat-1");
        assert!(f.contains(&"git_pr:create".to_string()));
        assert!(f.contains(&"git_branch:feat-1".to_string()));
        assert!(!f.iter().any(|t| t.starts_with("git_pr_num:")));
    }

    #[test]
    fn gh_pr_merge_number_from_command() {
        let f = git_facets_for_segment("gh pr merge 48 --squash");
        assert!(f.contains(&"git_pr:merge".to_string()));
        assert!(f.contains(&"git_pr_num:48".to_string()));
    }

    #[test]
    fn create_number_mined_from_output() {
        let cmd = git_facets_for_segment("gh pr create --fill");
        let mined = mine_output("https://github.com/o/r/pull/62\n", &cmd);
        assert!(mined.contains(&"git_pr_num:62".to_string()));
        assert!(mined.contains(&"git_pr:create-num:62".to_string()));
    }

    #[test]
    fn commit_sha_from_output_confirmation() {
        let cmd = git_facets_for_segment("git commit -m x");
        let mined = mine_output("[main abc1234] x\n 1 file changed\n", &cmd);
        assert_eq!(mined, vec!["git_commit:abc1234"]);
        // No commit facet when the command wasn't a commit.
        let mined2 = mine_output("[main abc1234] x", &[]);
        assert!(mined2.is_empty());
    }

    #[test]
    fn push_range_shas() {
        let mined = mine_output("   abc1234..def5678  main -> main", &[]);
        assert!(mined.contains(&"git_commit:abc1234".to_string()));
        assert!(mined.contains(&"git_commit:def5678".to_string()));
    }

    #[test]
    fn ssh_wrapped_git() {
        let f = git_facets_for_segment("ssh prod 'git pull origin main'");
        assert!(f.contains(&"git_cmd:pull".to_string()));
    }

    #[test]
    fn mutating_classification() {
        assert_eq!(is_mutating_facet("git_cmd:commit"), Some(true));
        assert_eq!(is_mutating_facet("git_cmd:status"), Some(false));
        assert_eq!(is_mutating_facet("git_pr:create"), Some(true));
        assert_eq!(is_mutating_facet("git_pr:view"), Some(false));
        assert_eq!(is_mutating_facet("tool:bash"), None);
    }

    #[test]
    fn global_flags_skipped() {
        assert_eq!(
            git_facets_for_segment("git -C /repo commit -m x"),
            vec!["git_cmd:commit"]
        );
    }
}
