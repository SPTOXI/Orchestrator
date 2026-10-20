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
//!
//! Versions: 1 — f32 tensors only (phase M1); 2 — adds the quantized
//! dtypes `q8`, `q4` and `t2` ([`crate::quant`]). The writer uses the
//! lowest version that can hold the file, so f32 models stay readable by
//! the oldest engines.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::fs::File;
use std::io::Write;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::mmap::Mmap;
use crate::quant;
use crate::tokenizer::TokenizerSpec;

pub const MAGIC: &[u8; 4] = b"NATV";
/// The newest format this engine reads and the one it writes.
pub const FORMAT_VERSION: u32 = 2;
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Dtype {
    F32,
    Q8,
    Q4,
    T2,
}

impl Dtype {
    pub fn is_quantized(self) -> bool {
        self != Dtype::F32
    }

    /// The first format version that has this dtype.
    pub fn since_version(self) -> u32 {
        match self {
            Dtype::F32 => 1,
            _ => 2,
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

    /// Size in bytes, or `None` when a quantized tensor's rows are not a
    /// whole number of blocks.
    pub fn bytes(&self) -> Option<usize> {
        let cols = *self.shape.last()?;
        if self.dtype.is_quantized() && !cols.is_multiple_of(quant::BLOCK) {
            return None;
        }
        let rows = self.elements().checked_div(cols).unwrap_or(0);
        rows.checked_mul(quant::row_bytes(self.dtype, cols))
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

enum Storage {
    Owned(Vec<u8>),
    Mapped(Mmap),
}

impl Storage {
    fn bytes(&self) -> &[u8] {
        match self {
            Storage::Owned(v) => v,
            Storage::Mapped(m) => m.as_slice(),
        }
    }
}

/// A `.nativa` file: the header, and the bytes either mapped from the disk
/// ([`NativaFile::open`]) or owned ([`NativaFile::parse`]).
pub struct NativaFile {
    pub version: u32,
    pub header: Header,
    storage: Storage,
    data_start: usize,
}

impl std::fmt::Debug for NativaFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NativaFile")
            .field("version", &self.version)
            .field("header", &self.header)
            .field("mapped", &self.is_mapped())
            .finish()
    }
}

impl NativaFile {
    /// Maps the file into memory (falls back to reading it when mapping is
    /// not possible).
    pub fn open(path: &Path) -> Result<Self> {
        let io = |source| Error::Io {
            path: path.to_path_buf(),
            source,
        };
        let file = File::open(path).map_err(io)?;
        match Mmap::map(&file) {
            Ok(map) => Self::from_storage(Storage::Mapped(map)),
            Err(_) => Self::parse(std::fs::read(path).map_err(io)?),
        }
    }

    /// Reads the whole file into memory.
    pub fn read(path: &Path) -> Result<Self> {
        let bytes = std::fs::read(path).map_err(|source| Error::Io {
            path: path.to_path_buf(),
            source,
        })?;
        Self::parse(bytes)
    }

    pub fn parse(bytes: Vec<u8>) -> Result<Self> {
        Self::from_storage(Storage::Owned(bytes))
    }

    pub fn is_mapped(&self) -> bool {
        matches!(self.storage, Storage::Mapped(_))
    }

    /// Bytes of tensor data (what memory mapping keeps on the disk).
    pub fn data_len(&self) -> usize {
        self.storage.bytes().len() - self.data_start
    }

    pub(crate) fn data(&self) -> &[u8] {
        &self.storage.bytes()[self.data_start..]
    }

    fn from_storage(storage: Storage) -> Result<Self> {
        let bytes = storage.bytes();
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
        let data = &bytes[data_start..];
        for t in &header.tensors {
            let bad = |reason: &str| Error::BadTensor {
                name: t.name.clone(),
                reason: reason.to_string(),
            };
            if t.dtype.since_version() > version {
                return Err(bad("dtype is newer than the file's format version"));
            }
            if !t.offset.is_multiple_of(ALIGN) {
                return Err(bad("offset is not aligned"));
            }
            let Some(len) = t.bytes() else {
                return Err(bad("rows are not a whole number of blocks"));
            };
            match t.offset.checked_add(len) {
                Some(end) if end <= data.len() => {}
                _ => return Err(bad("runs past the end of the file")),
            }
        }
        Ok(Self {
            version,
            header,
            storage,
            data_start,
        })
    }

    pub fn tensor_info(&self, name: &str) -> Option<&TensorInfo> {
        self.header.tensors.iter().find(|t| t.name == name)
    }

    /// The tensor's raw bytes, checked against the expected shape.
    pub fn tensor_bytes(&self, name: &str, shape: &[usize]) -> Result<(&TensorInfo, &[u8])> {
        let info = self
            .tensor_info(name)
            .ok_or_else(|| Error::MissingTensor(name.to_string()))?;
        if info.shape != shape {
            return Err(Error::BadTensor {
                name: name.to_string(),
                reason: format!("shape {:?}, expected {:?}", info.shape, shape),
            });
        }
        let len = info.bytes().expect("checked when the file was opened");
        Ok((info, &self.data()[info.offset..info.offset + len]))
    }

    /// The tensor as f32 values (quantized tensors are expanded).
    pub fn tensor_f32(&self, name: &str, shape: &[usize]) -> Result<Vec<f32>> {
        let (info, raw) = self.tensor_bytes(name, shape)?;
        let cols = *shape.last().unwrap_or(&1);
        let rows = info.elements().checked_div(cols).unwrap_or(0);
        let mut out = vec![0.0; info.elements()];
        let rb = quant::row_bytes(info.dtype, cols);
        for (r, o) in out.chunks_exact_mut(cols.max(1)).enumerate().take(rows) {
            quant::dequantize_row(info.dtype, &raw[r * rb..(r + 1) * rb], cols, o);
        }
        Ok(out)
    }
}

/// A tensor to write, already in its stored layout.
pub struct TensorData<'a> {
    pub name: &'a str,
    pub dtype: Dtype,
    pub shape: Vec<usize>,
    pub bytes: Cow<'a, [u8]>,
}

impl<'a> TensorData<'a> {
    pub fn f32(name: &'a str, shape: Vec<usize>, values: &[f32]) -> Self {
        Self::quantized(name, Dtype::F32, shape, values)
    }

    /// Quantizes `values` (row-major, rows along the last dimension).
    pub fn quantized(name: &'a str, dtype: Dtype, shape: Vec<usize>, values: &[f32]) -> Self {
        assert_eq!(
            shape.iter().product::<usize>(),
            values.len(),
            "tensor {name}: shape and values disagree"
        );
        let cols = *shape.last().expect("tensor without dimensions");
        let mut bytes =
            Vec::with_capacity(values.len() / cols.max(1) * quant::row_bytes(dtype, cols));
        for row in values.chunks_exact(cols.max(1)) {
            quant::quantize_row(dtype, row, &mut bytes);
        }
        Self {
            name,
            dtype,
            shape,
            bytes: Cow::Owned(bytes),
        }
    }
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
        let info = TensorInfo {
            name: t.name.to_string(),
            dtype: t.dtype,
            shape: t.shape.clone(),
            offset,
        };
        assert_eq!(
            info.bytes(),
            Some(t.bytes.len()),
            "tensor {}: shape, dtype and bytes disagree",
            t.name
        );
        index.push(info);
        offset = align_up(offset + t.bytes.len());
    }
    let version = tensors
        .iter()
        .map(|t| t.dtype.since_version())
        .max()
        .unwrap_or(1);
    let header = Header {
        config: config.clone(),
        tokenizer: tokenizer.clone(),
        metadata: metadata.clone(),
        tensors: index,
    };
    let json = serde_json::to_vec(&header).map_err(std::io::Error::other)?;
    out.write_all(MAGIC)?;
    out.write_all(&version.to_le_bytes())?;
    out.write_all(&(json.len() as u64).to_le_bytes())?;
    out.write_all(&json)?;
    let header_end = PREFIX + json.len();
    out.write_all(&vec![0; align_up(header_end) - header_end])?;
    let mut written = 0;
    for t in tensors {
        out.write_all(&t.bytes)?;
        written += t.bytes.len();
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
            TensorData::f32("a", vec![2, 3], &a),
            TensorData::f32("b", vec![3], &b),
        ]);
        let f = NativaFile::parse(bytes).unwrap();
        // f32 only: written as version 1, readable by the oldest engines.
        assert_eq!(f.version, 1);
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
    fn quantized_tensors_need_version_2() {
        let w: Vec<f32> = (0..64).map(|i| (i as f32 - 32.0) / 32.0).collect();
        let bytes = file(&[
            TensorData::quantized("w", Dtype::Q4, vec![2, 32], &w),
            TensorData::f32("n", vec![4], &[1.0; 4]),
        ]);
        let f = NativaFile::parse(bytes.clone()).unwrap();
        assert_eq!(f.version, 2);
        let back = f.tensor_f32("w", &[2, 32]).unwrap();
        assert!(w.iter().zip(&back).all(|(a, b)| (a - b).abs() < 0.08));

        // The same file claiming version 1 is refused, not misread.
        let mut old = bytes;
        old[4..8].copy_from_slice(&1u32.to_le_bytes());
        assert!(matches!(
            NativaFile::parse(old),
            Err(Error::BadTensor { .. })
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
        let mut bytes = file(&[TensorData::f32("a", vec![64], &a)]);
        bytes.truncate(bytes.len() - 8);
        assert!(matches!(
            NativaFile::parse(bytes),
            Err(Error::BadTensor { .. })
        ));
    }

    #[test]
    fn maps_files_from_disk() {
        let a: Vec<f32> = (0..100).map(|i| i as f32).collect();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("m.nativa");
        std::fs::write(&path, file(&[TensorData::f32("a", vec![100], &a)])).unwrap();
        let f = NativaFile::open(&path).unwrap();
        assert!(f.is_mapped());
        assert_eq!(f.tensor_f32("a", &[100]).unwrap(), a);
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
