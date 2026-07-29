//! Excerpt/highlight generation (port of agentpm's excerptForTerms):
//! center a window on the first match, UTF-8-safe boundaries, and shift
//! highlight ranges into excerpt coordinates.

pub const EXCERPT_MAX_CHARS: usize = 360;

#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct Highlight {
    pub start: usize,
    pub len: usize,
}

/// Byte-offset match ranges for each term (case-insensitive substring).
fn match_ranges(text: &str, terms: &[String]) -> Vec<(usize, usize)> {
    let lower = text.to_lowercase();
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    for term in terms {
        let term = term.to_lowercase();
        if term.is_empty() {
            continue;
        }
        let mut start = 0;
        while let Some(pos) = lower[start..].find(&term) {
            let begin = start + pos;
            // lower may differ in byte length from text for exotic case
            // mappings; guard by clamping to char boundaries later.
            ranges.push((begin, begin + term.len()));
            start = begin + term.len().max(1);
            if ranges.len() > 64 {
                break;
            }
        }
    }
    ranges.sort_unstable();
    ranges.dedup();
    ranges
}

fn utf8_floor(text: &str, mut idx: usize) -> usize {
    idx = idx.min(text.len());
    while idx > 0 && !text.is_char_boundary(idx) {
        idx -= 1;
    }
    idx
}

fn utf8_ceil(text: &str, mut idx: usize) -> usize {
    idx = idx.min(text.len());
    while idx < text.len() && !text.is_char_boundary(idx) {
        idx += 1;
    }
    idx
}

/// Build an excerpt window centered on the first match. Returns the excerpt
/// and highlight ranges in excerpt byte coordinates.
pub fn excerpt_for_terms(text: &str, terms: &[String], max_chars: usize) -> (String, Vec<Highlight>) {
    let text = text.trim();
    let ranges = match_ranges(text, terms);
    let window_start = match ranges.first() {
        Some(&(start, _)) => {
            let center = start;
            utf8_floor(text, center.saturating_sub(max_chars / 3))
        }
        None => 0,
    };
    let mut window_end = utf8_ceil(text, window_start + max_chars);
    // Extend a hair to avoid splitting the first matched term.
    if let Some(&(s, e)) = ranges.first() {
        if s >= window_start && e > window_end {
            window_end = utf8_ceil(text, e.min(window_start + max_chars * 2));
        }
    }
    let mut excerpt = String::new();
    if window_start > 0 {
        excerpt.push('…');
    }
    let prefix_len = excerpt.len();
    excerpt.push_str(&text[window_start..window_end]);
    if window_end < text.len() {
        excerpt.push('…');
    }
    let highlights = ranges
        .iter()
        .filter(|(s, e)| *s >= window_start && *e <= window_end)
        .map(|(s, e)| Highlight {
            start: s - window_start + prefix_len,
            len: e - s,
        })
        .collect();
    (excerpt.replace('\n', " "), highlights)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_text_passthrough() {
        let (e, h) = excerpt_for_terms("hello flaky world", &["flaky".into()], 360);
        assert_eq!(e, "hello flaky world");
        assert_eq!(h, vec![Highlight { start: 6, len: 5 }]);
    }

    #[test]
    fn long_text_centers_on_match() {
        let text = format!("{}NEEDLE{}", "x".repeat(2000), "y".repeat(2000));
        let (e, h) = excerpt_for_terms(&text, &["needle".into()], 120);
        assert!(e.contains("NEEDLE"));
        assert!(e.starts_with('…') && e.ends_with('…'));
        let hl = &h[0];
        assert_eq!(&e[hl.start..hl.start + hl.len], "NEEDLE");
    }

    #[test]
    fn utf8_boundaries_respected() {
        let text = format!("{}é needle {}", "日本語".repeat(300), "语".repeat(300));
        let (e, _h) = excerpt_for_terms(&text, &["needle".into()], 100);
        assert!(e.contains("needle"));
    }

    #[test]
    fn no_match_takes_head() {
        let text = "abc ".repeat(500);
        let (e, h) = excerpt_for_terms(&text, &["zzz".into()], 40);
        assert!(h.is_empty());
        assert!(e.len() < 60);
    }
}
