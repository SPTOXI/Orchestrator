//! Byte-level BPE (ADR-0027 §2a).
//!
//! Ids `0..256` are the bytes of UTF-8 text, so any language and script is
//! representable with no "unknown" token. Merge `k` joins a pair of ids into
//! id `256 + k`; merges apply in rank order (earliest learned first).
//! Special tokens (the Native AI's protocol: tools, results, routines…)
//! come after the merges and are matched literally before BPE runs.
//!
//! Byte-pair encoding as a tokenizer: Sennrich, Haddow and Birch, "Neural
//! Machine Translation of Rare Words with Subword Units" (2016).

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenizerSpec {
    /// In rank order; merge `k` produces id `256 + k`.
    pub merges: Vec<[u32; 2]>,
    /// Literal strings; special `j` is id `256 + merges.len() + j`.
    pub special: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct Tokenizer {
    spec: TokenizerSpec,
    ranks: HashMap<(u32, u32), u32>,
    /// The bytes behind every non-special id.
    pieces: Vec<Vec<u8>>,
}

impl Tokenizer {
    pub fn new(spec: TokenizerSpec) -> Result<Self> {
        let mut pieces: Vec<Vec<u8>> = (0..=255u8).map(|b| vec![b]).collect();
        let mut ranks = HashMap::with_capacity(spec.merges.len());
        for (k, &[a, b]) in spec.merges.iter().enumerate() {
            let next = pieces.len() as u32;
            if a >= next || b >= next {
                return Err(Error::BadHeader(format!(
                    "tokenizer merge {k} uses an id that does not exist yet"
                )));
            }
            if ranks.insert((a, b), k as u32).is_some() {
                return Err(Error::BadHeader(format!("tokenizer merge {k} is repeated")));
            }
            let mut piece = pieces[a as usize].clone();
            piece.extend_from_slice(&pieces[b as usize]);
            pieces.push(piece);
        }
        if spec.special.iter().any(String::is_empty) {
            return Err(Error::BadHeader("empty special token".into()));
        }
        Ok(Self {
            spec,
            ranks,
            pieces,
        })
    }

    pub fn spec(&self) -> &TokenizerSpec {
        &self.spec
    }

    pub fn vocab_size(&self) -> usize {
        self.pieces.len() + self.spec.special.len()
    }

    /// The id of a special token, by its text.
    pub fn special_id(&self, text: &str) -> Option<u32> {
        self.spec
            .special
            .iter()
            .position(|s| s == text)
            .map(|j| (self.pieces.len() + j) as u32)
    }

    pub fn encode(&self, text: &str) -> Vec<u32> {
        let mut out = Vec::new();
        let mut rest = text;
        while !rest.is_empty() {
            match self.next_special(rest) {
                Some((at, j, len)) => {
                    self.encode_plain(&rest[..at], &mut out);
                    out.push((self.pieces.len() + j) as u32);
                    rest = &rest[at + len..];
                }
                None => {
                    self.encode_plain(rest, &mut out);
                    break;
                }
            }
        }
        out
    }

    /// Text for the ids. Byte sequences that are not valid UTF-8 (a token
    /// cut in the middle of a character) become U+FFFD.
    pub fn decode(&self, ids: &[u32]) -> String {
        String::from_utf8_lossy(&self.decode_bytes(ids)).into_owned()
    }

    pub fn decode_bytes(&self, ids: &[u32]) -> Vec<u8> {
        let mut out = Vec::new();
        for &id in ids {
            let id = id as usize;
            if let Some(piece) = self.pieces.get(id) {
                out.extend_from_slice(piece);
            } else if let Some(s) = self.spec.special.get(id - self.pieces.len()) {
                out.extend_from_slice(s.as_bytes());
            }
        }
        out
    }

    /// The earliest special token in `text`: (byte offset, index, length).
    /// At the same offset the longest wins.
    fn next_special(&self, text: &str) -> Option<(usize, usize, usize)> {
        let mut best: Option<(usize, usize, usize)> = None;
        for (j, s) in self.spec.special.iter().enumerate() {
            if let Some(at) = text.find(s.as_str()) {
                let better = match best {
                    None => true,
                    Some((b_at, _, b_len)) => at < b_at || (at == b_at && s.len() > b_len),
                };
                if better {
                    best = Some((at, j, s.len()));
                }
            }
        }
        best
    }

    fn encode_plain(&self, text: &str, out: &mut Vec<u32>) {
        let mut ids: Vec<u32> = text.bytes().map(u32::from).collect();
        loop {
            let best = ids
                .windows(2)
                .filter_map(|w| self.ranks.get(&(w[0], w[1])).copied())
                .min();
            let Some(rank) = best else { break };
            let [a, b] = self.spec.merges[rank as usize];
            let merged = 256 + rank;
            let mut next = Vec::with_capacity(ids.len());
            let mut i = 0;
            while i < ids.len() {
                if i + 1 < ids.len() && ids[i] == a && ids[i + 1] == b {
                    next.push(merged);
                    i += 2;
                } else {
                    next.push(ids[i]);
                    i += 1;
                }
            }
            ids = next;
        }
        out.extend(ids);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tok() -> Tokenizer {
        // 256 = "e" + "s"; 257 = "t" + "es"... built from the bytes up.
        let e = u32::from(b'e');
        let s = u32::from(b's');
        let t = u32::from(b't');
        Tokenizer::new(TokenizerSpec {
            merges: vec![[e, s], [t, 256], [257, t]],
            special: vec!["<|ferramenta|>".into(), "<|f|>".into()],
        })
        .unwrap()
    }

    #[test]
    fn merges_in_rank_order() {
        let t = tok();
        assert_eq!(t.encode("test"), vec![258]);
        assert_eq!(t.encode("tes"), vec![257]);
        assert_eq!(t.encode("es es"), vec![256, u32::from(b' '), 256]);
        assert_eq!(t.vocab_size(), 256 + 3 + 2);
    }

    #[test]
    fn any_language_round_trips() {
        let t = tok();
        for text in ["olá, mundo", "こんにちは", "Привет", "مرحبا", "🙂 test", ""]
        {
            assert_eq!(t.decode(&t.encode(text)), text);
        }
    }

    #[test]
    fn special_tokens_are_literal() {
        let t = tok();
        let ids = t.encode("a<|ferramenta|>test<|f|>");
        assert_eq!(ids, vec![u32::from(b'a'), 259, 258, 260]);
        assert_eq!(t.special_id("<|f|>"), Some(260));
        assert_eq!(t.decode(&ids), "a<|ferramenta|>test<|f|>");
    }

    #[test]
    fn rejects_merges_from_the_future() {
        let r = Tokenizer::new(TokenizerSpec {
            merges: vec![[256, 1]],
            special: vec![],
        });
        assert!(r.is_err());
    }
}
