//! Minimal ANSI painter for CLI output.
//!
//! Color is enabled when stdout is a TTY, `NO_COLOR` is unset, and TERM is
//! not "dumb"; `CLICOLOR_FORCE=1` forces it on (tests, pagers).

use std::io::IsTerminal;

#[derive(Clone, Copy)]
pub struct Painter {
    enabled: bool,
}

pub const BOLD: &str = "1";
pub const DIM: &str = "2";
pub const RED: &str = "31";
pub const GREEN: &str = "32";
pub const YELLOW: &str = "33";
pub const BLUE: &str = "34";
pub const MAGENTA: &str = "35";
pub const CYAN: &str = "36";
pub const BOLD_YELLOW: &str = "1;33";
pub const BOLD_GREEN: &str = "1;32";
pub const BOLD_BLUE: &str = "1;34";
pub const HIGHLIGHT: &str = "1;30;43"; // black on yellow, like grep --color

impl Painter {
    pub fn auto() -> Painter {
        let force = std::env::var_os("CLICOLOR_FORCE").is_some_and(|v| v != "0");
        let no_color = std::env::var_os("NO_COLOR").is_some();
        let dumb = std::env::var_os("TERM").is_some_and(|t| t == "dumb");
        Painter {
            enabled: force || (std::io::stdout().is_terminal() && !no_color && !dumb),
        }
    }

    pub fn paint(&self, code: &str, text: &str) -> String {
        if self.enabled && !text.is_empty() {
            format!("\x1b[{code}m{text}\x1b[0m")
        } else {
            text.to_string()
        }
    }

    /// Paint byte ranges of `text` with `code` (used for excerpt match
    /// highlighting). Ranges must be sorted, non-overlapping, and on char
    /// boundaries; out-of-range highlights are skipped.
    pub fn paint_ranges(&self, text: &str, ranges: &[(usize, usize)], code: &str) -> String {
        if !self.enabled || ranges.is_empty() {
            return text.to_string();
        }
        let mut out = String::with_capacity(text.len() + ranges.len() * 12);
        let mut pos = 0;
        for &(start, end) in ranges {
            if start < pos || end > text.len() || start >= end {
                continue;
            }
            if !text.is_char_boundary(start) || !text.is_char_boundary(end) {
                continue;
            }
            out.push_str(&text[pos..start]);
            out.push_str(&format!("\x1b[{code}m"));
            out.push_str(&text[start..end]);
            out.push_str("\x1b[0m");
            pos = end;
        }
        out.push_str(&text[pos..]);
        out
    }

    /// Stable per-provider color so agents are tellable at a glance.
    pub fn provider(&self, provider: &str) -> String {
        let code = match provider {
            "claude" | "claude-cowork" => YELLOW,
            "codex" => GREEN,
            "cursor" => BLUE,
            "grok" => MAGENTA,
            "hermes" => RED,
            "opencode" => CYAN,
            _ => DIM,
        };
        self.paint(code, provider)
    }
}

/// Truncate to `max` chars on a char boundary, returning the byte length
/// kept (for filtering highlight ranges).
pub fn truncate_chars(text: &str, max: usize) -> (&str, usize) {
    match text.char_indices().nth(max) {
        Some((idx, _)) => (&text[..idx], idx),
        None => (text, text.len()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn forced() -> Painter {
        Painter { enabled: true }
    }

    #[test]
    fn paint_ranges_highlights() {
        let p = forced();
        let out = p.paint_ranges("find the needle here", &[(9, 15)], HIGHLIGHT);
        assert_eq!(out, "find the \x1b[1;30;43mneedle\x1b[0m here");
    }

    #[test]
    fn paint_ranges_skips_invalid() {
        let p = forced();
        // Overlapping and out-of-bounds ranges are dropped, not panicked on.
        let out = p.paint_ranges("abcdef", &[(1, 3), (2, 4), (5, 99)], DIM);
        assert_eq!(out, "a\x1b[2mbc\x1b[0mdef");
    }

    #[test]
    fn disabled_is_passthrough() {
        let p = Painter { enabled: false };
        assert_eq!(p.paint(BOLD, "x"), "x");
        assert_eq!(p.paint_ranges("abc", &[(0, 1)], DIM), "abc");
    }

    #[test]
    fn truncate_on_char_boundary() {
        let (s, len) = truncate_chars("日本語テキスト", 3);
        assert_eq!(s, "日本語");
        assert_eq!(len, 9);
    }
}
