//! The `.nativa` file (ADR-0027 §0 and §1): the project's own format.
//!
//! ```text
//! offset 0   "NATV"                       magic
//!        4   u32 LE                       format version
//!        8   u64 LE                       header length (bytes)
//!       16   header (UTF-8 JSON)          config, tokenizer, tensor index
//!            zero padding to ALIGN
//!            tensor data                  each tensor at an ALIGN offset
//! ```
//!
//! Tensor offsets are relative to the start of the data. Every engine reads
//! every earlier version of the format; a newer file is refused with a
//! clear error instead of being misread.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::tokenizer::TokenizerSpec;

pub const MAGIC: &[u8; 4] = b"NATV";
/// The newest format this engine reads and the one it writes.
pub const FORMAT_VERSION: u32 = 1;
/// Alignment of the data section and of each tensor (cache line, and
/// enough for any SIMD load later).
pub const ALIGN: usize = 64;
const PREFIX: usize = 16;

/// The shape of the model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Config {
    pub vocab_size: usize,
    pub dim: usize,
    pub n_layers: usize,
    pub n_heads: usize,
    pub n_kv_heads: usize,
    /// Width of the feed-forward layer.
    pub hidden_dim: usize,
    pub max_seq_len: usize,
    pub rope_theta: f32,
    pub norm_eps: f32,
}

impl Config {
    pub fn head_dim(&self) -> usize {
        self.dim / self.n_heads
    }

    pub fn kv_dim(&self) -> usize {
        self.n_kv_heads * self.head_dim()
    }

    pub fn validate(&self) -> Result<()> {
        let bad = |m: &str| Err(Error::BadConfig(m.to_string()));
        if self.vocab_size == 0 || self.dim == 0 || self.n_layers == 0 || self.hidden_dim == 0 {
            return bad("sizes must be positive");
        }
        if self.n_heads == 0 || !self.dim.is_multiple_of(self.n_heads) {
            return bad("dim must be a multiple of n_heads");
        }
        if !self.head_dim().is_multiple_of(2) {
            return bad("head_dim must be even (rotary embeddings rotate pairs)");
        }
        if self.n_kv_heads == 0 || !self.n_heads.is_multiple_of(self.n_kv_heads) {
            return bad("n_heads must be a multiple of n_kv_heads");
        }
        if self.max_seq_len == 0 {
            return bad("max_seq_len must be positive");
        }
        let positive = |v: f32| v.is_finite() && v > 0.0;
        if !positive(self.rope_theta) || !positive(self.norm_eps) {
            return bad("rope_theta and norm_eps must be positive");
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Dtype {
    F32,
}

impl Dtype {
    pub fn size(self) -> usize {
        match self {
            Dtype::F32 => 4,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TensorInfo {
    pub name: String,
    pub dtype: Dtype,
    pub shape: Vec<usize>,
    /// From the start of the data section.
    pub offset: usize,
}

impl TensorInfo {
    pub fn elements(&self) -> usize {
        self.shape.iter().product()
    }

    pub fn bytes(&self) -> usize {
        self.elements() * self.dtype.size()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Header {
    pub config: Config,
    pub tokenizer: TokenizerSpec,
    /// Free text: name, version, training notes.
    #[serde(default)]
    pub metadata: BTreeMap<String, String>,
    pub tensors: Vec<TensorInfo>,
}

/// A `.nativa` file in memory.
#[derive(Debug)]
pub struct NativaFile {
    pub version: u32,
    pub header: Header,
    data: Vec<u8>,
}

impl NativaFile {
    pub fn read(path: &Path) -> Result<Self> {
        let bytes = std::fs::read(path).map_err(|source| Error::Io {
            path: path.to_path_buf(),
            source,
        })?;
        Self::parse(bytes)
    }

    pub fn parse(bytes: Vec<u8>) -> Result<Self> {
        if bytes.len() < PREFIX || &bytes[..4] != MAGIC {
            return Err(Error::BadMagic);
        }
        let version = u32::from_le_bytes(bytes[4..8].try_into().unwrap());
        if version == 0 || version > FORMAT_VERSION {
            return Err(Error::UnsupportedVersion {
                found: version,
                supported: FORMAT_VERSION,
            });
        }
        let header_len = u64::from_le_bytes(bytes[8..16].try_into().unwrap());
        let header_end = usize::try_from(header_len)
            .ok()
            .and_then(|n| PREFIX.checked_add(n))
            .filter(|&end| end <= bytes.len())
            .ok_or_else(|| Error::BadHeader("header runs past the end of the file".into()))?;
        let header: Header = serde_json::from_slice(&bytes[PREFIX..header_end])
            .map_err(|e| Error::BadHeader(e.to_string()))?;
        let data_start = align_up(header_end);
        if data_start > bytes.len() {
            return Err(Error::BadHeader("no data section".into()));
        }
        let data = bytes[data_start..].to_vec();
        for t in &header.tensors {
            let bad = |reason: &str| Error::BadTensor {
                name: t.name.clone(),
                reason: reason.to_string(),
            };
            if !t.offset.is_multiple_of(ALIGN) {
                return Err(bad("offset is not aligned"));
            }
            match t.offset.checked_add(t.bytes()) {
                Some(end) if end <= data.len() => {}
                _ => return Err(bad("runs past the end of the file")),
            }
        }
        Ok(Self {
            version,
            header,
            data,
        })
    }

    pub fn tensor_info(&self, name: &str) -> Option<&TensorInfo> {
        self.header.tensors.iter().find(|t| t.name == name)
    }

    /// The tensor as f32 values, checked against the expected shape.
    pub fn tensor_f32(&self, name: &str, shape: &[usize]) -> Result<Vec<f32>> {
        let info = self
            .tensor_info(name)
            .ok_or_else(|| Error::MissingTensor(name.to_string()))?;
        if info.shape != shape {
            return Err(Error::BadTensor {
                name: name.to_string(),
                reason: format!("shape {:?}, expected {:?}", info.shape, shape),
            });
        }
        let raw = &self.data[info.offset..info.offset + info.bytes()];
        Ok(match info.dtype {
            Dtype::F32 => raw
                .chunks_exact(4)
                .map(|c| f32::from_le_bytes(c.try_into().unwrap()))
                .collect(),
        })
    }
}

/// A tensor to write.
pub struct TensorData<'a> {
    pub name: &'a str,
    pub shape: Vec<usize>,
    pub values: &'a [f32],
}

/// Writes a `.nativa` file. The tensor index in `header` is replaced by the
/// one built from `tensors`.
pub fn write(
    out: &mut impl Write,
    config: &Config,
    tokenizer: &TokenizerSpec,
    metadata: &BTreeMap<String, String>,
    tensors: &[TensorData<'_>],
) -> std::io::Result<()> {
    let mut index = Vec::with_capacity(tensors.len());
    let mut offset = 0;
    for t in tensors {
        assert_eq!(
            t.shape.iter().product::<usize>(),
            t.values.len(),
            "tensor {}: shape and values disagree",
            t.name
        );
        index.push(TensorInfo {
            name: t.name.to_string(),
            dtype: Dtype::F32,
            shape: t.shape.clone(),
            offset,
        });
        offset = align_up(offset + t.values.len() * 4);
    }
    let header = Header {
        config: config.clone(),
        tokenizer: tokenizer.clone(),
        metadata: metadata.clone(),
        tensors: index,
    };
    let json = serde_json::to_vec(&header).map_err(std::io::Error::other)?;
    out.write_all(MAGIC)?;
    out.write_all(&FORMAT_VERSION.to_le_bytes())?;
    out.write_all(&(json.len() as u64).to_le_bytes())?;
    out.write_all(&json)?;
    let header_end = PREFIX + json.len();
    out.write_all(&vec![0; align_up(header_end) - header_end])?;
    let mut written = 0;
    for t in tensors {
        for v in t.values {
            out.write_all(&v.to_le_bytes())?;
        }
        written += t.values.len() * 4;
        let pad = align_up(written) - written;
        out.write_all(&vec![0; pad])?;
        written += pad;
    }
    Ok(())
}

fn align_up(n: usize) -> usize {
    n.div_ceil(ALIGN) * ALIGN
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> Config {
        Config {
            vocab_size: 258,
            dim: 8,
            n_layers: 1,
            n_heads: 2,
            n_kv_heads: 1,
            hidden_dim: 16,
            max_seq_len: 32,
            rope_theta: 10000.0,
            norm_eps: 1e-5,
        }
    }

    fn file(tensors: &[TensorData<'_>]) -> Vec<u8> {
        let mut out = Vec::new();
        let tok = TokenizerSpec {
            merges: vec![],
            special: vec!["<|a|>".into(), "<|b|>".into()],
        };
        write(&mut out, &config(), &tok, &BTreeMap::new(), tensors).unwrap();
        out
    }

    #[test]
    fn round_trip() {
        let a: Vec<f32> = (0..6).map(|i| i as f32 * 0.5).collect();
        let b = vec![-1.0f32; 3];
        let bytes = file(&[
            TensorData {
                name: "a",
                shape: vec![2, 3],
                values: &a,
            },
            TensorData {
                name: "b",
                shape: vec![3],
                values: &b,
            },
        ]);
        let f = NativaFile::parse(bytes).unwrap();
        assert_eq!(f.version, FORMAT_VERSION);
        assert_eq!(f.header.config, config());
        assert_eq!(f.tensor_f32("a", &[2, 3]).unwrap(), a);
        assert_eq!(f.tensor_f32("b", &[3]).unwrap(), b);
        assert!(f.header.tensors.iter().all(|t| t.offset % ALIGN == 0));
        assert!(matches!(
            f.tensor_f32("a", &[3, 2]),
            Err(Error::BadTensor { .. })
        ));
        assert!(matches!(
            f.tensor_f32("c", &[1]),
            Err(Error::MissingTensor(_))
        ));
    }

    #[test]
    fn refuses_other_files_and_newer_versions() {
        assert!(matches!(
            NativaFile::parse(b"GGUF\0\0\0\0".to_vec()),
            Err(Error::BadMagic)
        ));
        let mut bytes = file(&[]);
        bytes[4..8].copy_from_slice(&(FORMAT_VERSION + 1).to_le_bytes());
        assert!(matches!(
            NativaFile::parse(bytes),
            Err(Error::UnsupportedVersion { .. })
        ));
    }

    #[test]
    fn refuses_truncated_tensors() {
        let a = vec![1.0f32; 64];
        let mut bytes = file(&[TensorData {
            name: "a",
            shape: vec![64],
            values: &a,
        }]);
        bytes.truncate(bytes.len() - 8);
        assert!(matches!(
            NativaFile::parse(bytes),
            Err(Error::BadTensor { .. })
        ));
    }

    #[test]
    fn config_rules() {
        assert!(config().validate().is_ok());
        let mut c = config();
        c.n_kv_heads = 3;
        assert!(c.validate().is_err());
        let mut c = config();
        c.dim = 6;
        c.n_heads = 2; // head_dim 3 is odd
        assert!(c.validate().is_err());
    }
}
