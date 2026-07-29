//! Byte-accurate JSONL line reader.
//!
//! Invariant: line numbers and byte offsets advance for EVERY physical line,
//! including malformed and oversized ones, so a checkpoint taken after N
//! lines is exact regardless of garbage in the prefix.

use std::io::{self, BufRead};

/// Max accepted line length (matches agentpm's 16 MiB scanner cap).
pub const MAX_LINE_BYTES: usize = 16 * 1024 * 1024;

pub struct RawLine {
    pub bytes: Vec<u8>,
    /// 1-based line number.
    pub line: u64,
    pub byte_start: u64,
    /// Exclusive end offset (includes the newline when present).
    pub byte_end: u64,
    /// Line exceeded MAX_LINE_BYTES; bytes are truncated and the line must be
    /// treated as malformed.
    pub oversized: bool,
}

pub struct LineReader<R: BufRead> {
    inner: R,
    next_line: u64,
    offset: u64,
}

impl<R: BufRead> LineReader<R> {
    /// `start_line` is the 1-based number the next line will get;
    /// `start_offset` its byte offset.
    pub fn new(inner: R, start_line: u64, start_offset: u64) -> Self {
        LineReader {
            inner,
            next_line: start_line,
            offset: start_offset,
        }
    }

    pub fn next_line(&mut self) -> io::Result<Option<RawLine>> {
        let mut buf: Vec<u8> = Vec::new();
        let mut oversized = false;
        let mut consumed: u64 = 0;
        loop {
            let chunk = self.inner.fill_buf()?;
            if chunk.is_empty() {
                break; // EOF
            }
            match memchr(b'\n', chunk) {
                Some(pos) => {
                    let take = pos + 1;
                    if buf.len() < MAX_LINE_BYTES {
                        let room = MAX_LINE_BYTES - buf.len();
                        buf.extend_from_slice(&chunk[..pos.min(room)]);
                    }
                    if pos > MAX_LINE_BYTES {
                        oversized = true;
                    }
                    self.inner.consume(take);
                    consumed += take as u64;
                    break;
                }
                None => {
                    let len = chunk.len();
                    if buf.len() < MAX_LINE_BYTES {
                        let room = MAX_LINE_BYTES - buf.len();
                        buf.extend_from_slice(&chunk[..len.min(room)]);
                    }
                    if buf.len() >= MAX_LINE_BYTES {
                        oversized = true;
                    }
                    self.inner.consume(len);
                    consumed += len as u64;
                }
            }
        }
        if consumed == 0 && buf.is_empty() {
            return Ok(None);
        }
        // Strip trailing \r for CRLF files.
        if buf.last() == Some(&b'\r') {
            buf.pop();
        }
        let line = RawLine {
            bytes: buf,
            line: self.next_line,
            byte_start: self.offset,
            byte_end: self.offset + consumed,
            oversized,
        };
        self.next_line += 1;
        self.offset += consumed;
        Ok(Some(line))
    }
}

fn memchr(needle: u8, haystack: &[u8]) -> Option<usize> {
    haystack.iter().position(|&b| b == needle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn lines(input: &[u8]) -> Vec<RawLine> {
        let mut r = LineReader::new(Cursor::new(input.to_vec()), 1, 0);
        let mut out = Vec::new();
        while let Some(l) = r.next_line().unwrap() {
            out.push(l);
        }
        out
    }

    #[test]
    fn offsets_are_exact() {
        let ls = lines(b"abc\ndefg\n\nx");
        assert_eq!(ls.len(), 4);
        assert_eq!((ls[0].byte_start, ls[0].byte_end), (0, 4));
        assert_eq!((ls[1].byte_start, ls[1].byte_end), (4, 9));
        assert_eq!((ls[2].byte_start, ls[2].byte_end), (9, 10));
        assert_eq!((ls[3].byte_start, ls[3].byte_end), (10, 11)); // no trailing newline
        assert_eq!(ls[3].line, 4);
        assert_eq!(ls[3].bytes, b"x");
    }

    #[test]
    fn crlf_stripped_offsets_keep_crlf() {
        let ls = lines(b"ab\r\ncd\r\n");
        assert_eq!(ls[0].bytes, b"ab");
        assert_eq!(ls[0].byte_end, 4);
        assert_eq!(ls[1].byte_start, 4);
    }

    #[test]
    fn resume_mid_file() {
        let input = b"aaa\nbbb\nccc\n";
        let mut r = LineReader::new(Cursor::new(&input[8..]), 3, 8);
        let l = r.next_line().unwrap().unwrap();
        assert_eq!(l.bytes, b"ccc");
        assert_eq!(l.line, 3);
        assert_eq!(l.byte_start, 8);
    }
}
