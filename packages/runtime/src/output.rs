//! Bounded output buffers addressed by absolute byte offsets, and an
//! incremental UTF-8 decoder for byte streams (PTY and pipe chunks).
//!
//! Offsets let a reader (the UI after a reload, or an AI agent) fetch only the
//! output produced since its last read: `read(since) -> { data, next }`.

use serde::Serialize;

/// Bytes of output kept per terminal/process (ADR-0004).
pub const DEFAULT_BUFFER_CAPACITY: usize = 1024 * 1024;

/// A window of output returned by [`OutputBuffer::read`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OutputChunk {
    pub data: String,
    /// Offset of the first byte of `data`.
    pub from: u64,
    /// Offset to pass as `since` on the next read.
    pub next: u64,
    /// True when output between the requested `since` and `from` was dropped
    /// because the buffer is bounded.
    pub truncated: bool,
    /// True when more output is already available after `next`.
    pub has_more: bool,
}

/// Keeps the most recent `capacity` bytes of a text stream.
#[derive(Debug, Clone)]
pub struct OutputBuffer {
    text: String,
    /// Absolute offset of `text[0]` in the whole stream.
    start: u64,
    capacity: usize,
}

impl Default for OutputBuffer {
    fn default() -> Self {
        Self::new(DEFAULT_BUFFER_CAPACITY)
    }
}

impl OutputBuffer {
    pub fn new(capacity: usize) -> Self {
        Self {
            text: String::new(),
            start: 0,
            capacity: capacity.max(4),
        }
    }

    /// Absolute offset right after the last byte written.
    pub fn end(&self) -> u64 {
        self.start + self.text.len() as u64
    }

    /// Absolute offset of the oldest byte still available.
    pub fn start(&self) -> u64 {
        self.start
    }

    /// Appends text and returns the absolute offset where it starts.
    pub fn push(&mut self, text: &str) -> u64 {
        let offset = self.end();
        self.text.push_str(text);
        if self.text.len() > self.capacity {
            let mut cut = self.text.len() - self.capacity;
            while !self.text.is_char_boundary(cut) {
                cut += 1;
            }
            self.text.drain(..cut);
            self.start += cut as u64;
        }
        offset
    }

    /// Reads output starting at `since` (default: oldest available byte), at
    /// most `max_bytes` bytes (default: everything available).
    pub fn read(&self, since: Option<u64>, max_bytes: Option<usize>) -> OutputChunk {
        let end = self.end();
        let requested = since.unwrap_or(self.start);
        let truncated = requested < self.start;
        let from = requested.clamp(self.start, end);

        let mut begin = (from - self.start) as usize;
        while !self.text.is_char_boundary(begin) {
            begin += 1;
        }

        let len = self.text.len();
        let mut stop = len;
        if let Some(max) = max_bytes {
            if len - begin > max {
                stop = begin + max;
                while stop > begin && !self.text.is_char_boundary(stop) {
                    stop -= 1;
                }
                if stop == begin && max > 0 {
                    // `max` is smaller than one character: return that character
                    // so the reader always makes progress.
                    stop = begin + 1;
                    while !self.text.is_char_boundary(stop) {
                        stop += 1;
                    }
                }
            }
        }

        OutputChunk {
            data: self.text[begin..stop].to_owned(),
            from: self.start + begin as u64,
            next: self.start + stop as u64,
            truncated,
            has_more: stop < len,
        }
    }
}

/// Decodes UTF-8 from arbitrary byte chunks, keeping incomplete trailing
/// sequences until the next chunk arrives. Invalid bytes become U+FFFD.
#[derive(Debug, Default)]
pub struct Utf8Decoder {
    pending: Vec<u8>,
}

impl Utf8Decoder {
    pub fn decode(&mut self, input: &[u8]) -> String {
        let mut bytes = std::mem::take(&mut self.pending);
        bytes.extend_from_slice(input);

        let mut out = String::with_capacity(bytes.len());
        let mut rest: &[u8] = &bytes;
        loop {
            match std::str::from_utf8(rest) {
                Ok(valid) => {
                    out.push_str(valid);
                    break;
                }
                Err(err) => {
                    let valid = err.valid_up_to();
                    // `valid_up_to` guarantees this prefix is valid UTF-8.
                    out.push_str(std::str::from_utf8(&rest[..valid]).unwrap_or_default());
                    match err.error_len() {
                        Some(invalid) => {
                            out.push(char::REPLACEMENT_CHARACTER);
                            rest = &rest[valid + invalid..];
                        }
                        None => {
                            self.pending = rest[valid..].to_vec();
                            break;
                        }
                    }
                }
            }
        }
        out
    }

    /// Flushes bytes still pending at end of stream.
    pub fn finish(&mut self) -> String {
        let pending = std::mem::take(&mut self.pending);
        String::from_utf8_lossy(&pending).into_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_returns_offsets_and_read_is_incremental() {
        let mut buf = OutputBuffer::new(1024);
        assert_eq!(buf.push("hello "), 0);
        assert_eq!(buf.push("world"), 6);

        let all = buf.read(None, None);
        assert_eq!(all.data, "hello world");
        assert_eq!((all.from, all.next), (0, 11));
        assert!(!all.truncated && !all.has_more);

        buf.push("!");
        let new = buf.read(Some(all.next), None);
        assert_eq!(new.data, "!");
        assert_eq!(new.next, 12);
    }

    #[test]
    fn capacity_drops_oldest_bytes_and_reports_truncation() {
        let mut buf = OutputBuffer::new(8);
        buf.push("0123456789");
        assert_eq!(buf.start(), 2);
        let chunk = buf.read(Some(0), None);
        assert_eq!(chunk.data, "23456789");
        assert!(chunk.truncated);
        assert_eq!(chunk.from, 2);
    }

    #[test]
    fn capacity_never_splits_characters() {
        let mut buf = OutputBuffer::new(5);
        buf.push("aéééé"); // 1 + 4*2 = 9 bytes
        let chunk = buf.read(None, None);
        assert!(chunk.data.chars().all(|c| c == 'é'));
        assert!(chunk.data.len() <= 5);
    }

    #[test]
    fn max_bytes_limits_and_signals_more() {
        let mut buf = OutputBuffer::new(1024);
        buf.push("abcdef");
        let first = buf.read(None, Some(4));
        assert_eq!(first.data, "abcd");
        assert!(first.has_more);
        let second = buf.read(Some(first.next), Some(4));
        assert_eq!(second.data, "ef");
        assert!(!second.has_more);
    }

    #[test]
    fn max_bytes_smaller_than_a_char_still_progresses() {
        let mut buf = OutputBuffer::new(1024);
        buf.push("€x");
        let chunk = buf.read(None, Some(1));
        assert_eq!(chunk.data, "€");
        assert_eq!(chunk.next, 3);
    }

    #[test]
    fn since_beyond_end_returns_empty() {
        let mut buf = OutputBuffer::new(1024);
        buf.push("abc");
        let chunk = buf.read(Some(99), None);
        assert_eq!(chunk.data, "");
        assert_eq!(chunk.next, 3);
    }

    #[test]
    fn decoder_joins_split_multibyte_sequences() {
        let bytes = "ação €".as_bytes();
        let mut dec = Utf8Decoder::default();
        let mut out = String::new();
        for b in bytes {
            out.push_str(&dec.decode(std::slice::from_ref(b)));
        }
        out.push_str(&dec.finish());
        assert_eq!(out, "ação €");
    }

    #[test]
    fn decoder_replaces_invalid_bytes() {
        let mut dec = Utf8Decoder::default();
        assert_eq!(dec.decode(b"a\xffb"), "a\u{FFFD}b");
        assert_eq!(dec.decode(b"\xe2\x82"), "");
        assert_eq!(dec.finish(), "\u{FFFD}");
    }
}
