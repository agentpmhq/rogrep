//! Shell command segmentation (port of agentpm tooltree/shell.go).
//!
//! `split_segments` splits a command line on `&&`, `||`, `|`, `;`, and
//! newlines while respecting quotes and escapes, recording the operator
//! that preceded each segment. Heredoc bodies are stripped first so their
//! content never reads as commands. `bash -c '…'` wrappers are expanded.

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Segment {
    pub text: String,
    /// Operator before this segment: "", "&&", "||", "|", ";", "\n".
    pub operator_before: String,
}

/// Strip heredoc bodies (`<<EOF … EOF`, `<<'EOF' … EOF`, `<<-EOF`) so the
/// body lines never look like commands.
pub fn strip_heredoc_bodies(script: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    let mut lines = script.lines();
    while let Some(line) = lines.next() {
        out.push(line);
        if let Some(tag) = heredoc_tag(line) {
            for body_line in lines.by_ref() {
                if body_line.trim() == tag {
                    break;
                }
            }
        }
    }
    out.join("\n")
}

fn heredoc_tag(line: &str) -> Option<String> {
    let idx = line.find("<<")?;
    let rest = &line[idx + 2..];
    let rest = rest.strip_prefix('-').unwrap_or(rest);
    let rest = rest.trim_start();
    let tag: String = if let Some(stripped) = rest.strip_prefix('\'') {
        stripped.chars().take_while(|c| *c != '\'').collect()
    } else if let Some(stripped) = rest.strip_prefix('"') {
        stripped.chars().take_while(|c| *c != '"').collect()
    } else {
        rest.chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect()
    };
    if tag.is_empty() {
        None
    } else {
        Some(tag)
    }
}

/// Split on shell operators, respecting single/double quotes, backslash
/// escapes, and backticks.
pub fn split_segments(command: &str) -> Vec<Segment> {
    let script = strip_heredoc_bodies(command);
    let mut segments = Vec::new();
    let mut current = String::new();
    let mut operator = String::new();
    let mut chars = script.chars().peekable();
    let (mut in_single, mut in_double, mut in_backtick) = (false, false, false);

    let flush = |current: &mut String, operator: &mut String, next_op: &str, segments: &mut Vec<Segment>| {
        let text = current.trim().to_string();
        if !text.is_empty() {
            segments.push(Segment {
                text,
                operator_before: operator.clone(),
            });
        }
        current.clear();
        *operator = next_op.to_string();
    };

    while let Some(c) = chars.next() {
        match c {
            '\\' if !in_single => {
                current.push(c);
                if let Some(next) = chars.next() {
                    current.push(next);
                }
            }
            '\'' if !in_double && !in_backtick => {
                in_single = !in_single;
                current.push(c);
            }
            '"' if !in_single && !in_backtick => {
                in_double = !in_double;
                current.push(c);
            }
            '`' if !in_single => {
                in_backtick = !in_backtick;
                current.push(c);
            }
            '&' if !in_single && !in_double && !in_backtick && chars.peek() == Some(&'&') => {
                chars.next();
                flush(&mut current, &mut operator, "&&", &mut segments);
            }
            '|' if !in_single && !in_double && !in_backtick => {
                if chars.peek() == Some(&'|') {
                    chars.next();
                    flush(&mut current, &mut operator, "||", &mut segments);
                } else {
                    flush(&mut current, &mut operator, "|", &mut segments);
                }
            }
            ';' if !in_single && !in_double && !in_backtick => {
                flush(&mut current, &mut operator, ";", &mut segments);
            }
            '\n' if !in_single && !in_double && !in_backtick => {
                flush(&mut current, &mut operator, "\n", &mut segments);
            }
            c => current.push(c),
        }
    }
    flush(&mut current, &mut operator, "", &mut segments);
    segments
}

/// Unwrap `bash -c '…'` / `sh -lc "…"` wrappers into their inner script.
pub fn expand_wrapper(segment: &str) -> Option<String> {
    let fields = fields(segment);
    if fields.len() < 3 {
        return None;
    }
    let exe = base_name(&fields[0]);
    if !matches!(exe.as_str(), "bash" | "sh" | "zsh" | "dash") {
        return None;
    }
    let mut saw_c = false;
    for f in &fields[1..] {
        if f.starts_with('-') && f.contains('c') {
            saw_c = true;
        } else if saw_c {
            return Some(f.clone());
        }
    }
    None
}

/// Field-split one segment: shell-words when it parses, whitespace fallback
/// for malformed commands (extraction must never fail).
pub fn fields(segment: &str) -> Vec<String> {
    match shell_words::split(segment) {
        Ok(f) => f,
        Err(_) => segment.split_whitespace().map(|s| s.to_string()).collect(),
    }
}

pub fn base_name(path: &str) -> String {
    path.rsplit('/').next().unwrap_or(path).to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_on_operators() {
        let segs = split_segments("cargo build && cargo test || echo fail; ls | head");
        let texts: Vec<&str> = segs.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(texts, vec!["cargo build", "cargo test", "echo fail", "ls", "head"]);
        let ops: Vec<&str> = segs.iter().map(|s| s.operator_before.as_str()).collect();
        assert_eq!(ops, vec!["", "&&", "||", ";", "|"]);
    }

    #[test]
    fn quotes_protect_operators() {
        let segs = split_segments("echo 'a && b' | grep \"x|y\"");
        assert_eq!(segs.len(), 2);
        assert_eq!(segs[0].text, "echo 'a && b'");
        assert_eq!(segs[1].text, "grep \"x|y\"");
    }

    #[test]
    fn heredoc_bodies_stripped() {
        let script = "cat > f <<'EOF'\ngit push --force\nEOF\ngit status";
        let segs = split_segments(script);
        let texts: Vec<&str> = segs.iter().map(|s| s.text.as_str()).collect();
        assert!(texts.contains(&"git status"));
        assert!(!texts.iter().any(|t| t.contains("--force")));
    }

    #[test]
    fn wrapper_expansion() {
        assert_eq!(
            expand_wrapper("bash -c 'git commit -m hi'"),
            Some("git commit -m hi".to_string())
        );
        assert_eq!(expand_wrapper("bash -lc \"ls\""), Some("ls".to_string()));
        assert_eq!(expand_wrapper("python -c 'print(1)'"), None);
    }

    #[test]
    fn malformed_never_panics() {
        let segs = split_segments("echo 'unterminated");
        assert!(!segs.is_empty());
        let f = fields("echo 'unterminated");
        assert_eq!(f, vec!["echo", "'unterminated"]);
    }
}
