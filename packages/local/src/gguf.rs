//! What a GGUF file says about itself (ADR-0025): architecture, name,
//! training context, quantization, the shape needed to size the KV cache,
//! and the chat template, which tells whether the model takes tools.
//!
//! Only the header is read. Large arrays (the tokenizer's vocabulary) are
//! skipped, not loaded.

use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::Path;

const MAGIC: &[u8; 4] = b"GGUF";
/// Longest string kept (chat templates are a few KB; anything bigger is
/// not one we need).
const MAX_STRING: u64 = 1024 * 1024;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct GgufInfo {
    pub architecture: Option<String>,
    pub name: Option<String>,
    /// "8B", "4B"…, when the file says.
    pub size_label: Option<String>,
    /// "Q4_K_M", "Q8_0"…
    pub quantization: Option<String>,
    /// Context the model was trained for (tokens).
    pub context_length: Option<u64>,
    pub block_count: Option<u64>,
    pub embedding_length: Option<u64>,
    pub head_count: Option<u64>,
    pub head_count_kv: Option<u64>,
    pub key_length: Option<u64>,
    pub value_length: Option<u64>,
    pub chat_template: Option<String>,
}

impl GgufInfo {
    /// The chat template handles tool definitions: the server can pass
    /// them natively (`--jinja`).
    pub fn takes_tools(&self) -> bool {
        self.chat_template
            .as_deref()
            .is_some_and(template_takes_tools)
    }

    /// Bytes of KV cache per token of context (f16 keys and values).
    pub fn kv_bytes_per_token(&self) -> Option<u64> {
        let layers = self.block_count?;
        let heads = self.head_count.filter(|h| *h > 0)?;
        let kv_heads = self.head_count_kv.unwrap_or(heads);
        let head_dim = self.embedding_length? / heads;
        let k = self.key_length.unwrap_or(head_dim);
        let v = self.value_length.unwrap_or(head_dim);
        Some(layers * kv_heads * (k + v) * 2)
    }
}

/// Reads the header of `path`.
pub fn read(path: &Path) -> Result<GgufInfo, String> {
    let file =
        File::open(path).map_err(|e| format!("não foi possível abrir {}: {e}", path.display()))?;
    let mut r = BufReader::with_capacity(256 * 1024, file);
    let mut magic = [0u8; 4];
    r.read_exact(&mut magic)
        .map_err(|_| "arquivo curto demais para ser um modelo GGUF".to_owned())?;
    if &magic != MAGIC {
        return Err("o arquivo não é um modelo GGUF".into());
    }
    let version = u32(&mut r)?;
    if !(2..=3).contains(&version) {
        return Err(format!("versão {version} do formato GGUF não suportada"));
    }
    let _tensors = u64(&mut r)?;
    let kv_count = u64(&mut r)?;
    let mut keys: Vec<(String, Value)> = Vec::new();
    for _ in 0..kv_count {
        let key = string(&mut r)?.unwrap_or_default();
        let kind = u32(&mut r)?;
        let wanted = wanted(&key);
        let value = value(&mut r, kind, wanted)?;
        if wanted {
            keys.push((key, value));
        }
    }
    let get = |name: &str| keys.iter().find(|(k, _)| k == name).map(|(_, v)| v);
    let arch = get("general.architecture").and_then(Value::text);
    let per_arch = |suffix: &str| {
        arch.as_ref()
            .and_then(|a| get(&format!("{a}.{suffix}")))
            .and_then(Value::number)
    };
    Ok(GgufInfo {
        name: get("general.name").and_then(Value::text),
        size_label: get("general.size_label").and_then(Value::text),
        quantization: get("general.file_type")
            .and_then(Value::number)
            .and_then(file_type),
        context_length: per_arch("context_length"),
        block_count: per_arch("block_count"),
        embedding_length: per_arch("embedding_length"),
        head_count: per_arch("attention.head_count"),
        head_count_kv: per_arch("attention.head_count_kv"),
        key_length: per_arch("attention.key_length"),
        value_length: per_arch("attention.value_length"),
        chat_template: get("tokenizer.chat_template").and_then(Value::text),
        architecture: arch,
    })
}

fn wanted(key: &str) -> bool {
    matches!(
        key,
        "general.architecture"
            | "general.name"
            | "general.size_label"
            | "general.file_type"
            | "tokenizer.chat_template"
    ) || [
        ".context_length",
        ".block_count",
        ".embedding_length",
        ".attention.head_count",
        ".attention.head_count_kv",
        ".attention.key_length",
        ".attention.value_length",
    ]
    .iter()
    .any(|suffix| key.ends_with(suffix) && !key.starts_with("general."))
}

#[derive(Debug, Clone)]
enum Value {
    Number(u64),
    Text(String),
    Other,
}

impl Value {
    fn number(&self) -> Option<u64> {
        match self {
            Value::Number(n) => Some(*n),
            _ => None,
        }
    }
    fn text(&self) -> Option<String> {
        match self {
            Value::Text(t) => Some(t.clone()),
            _ => None,
        }
    }
}

type R = BufReader<File>;

fn io(e: std::io::Error) -> String {
    format!("cabeçalho GGUF incompleto: {e}")
}

fn u32(r: &mut R) -> Result<u32, String> {
    let mut b = [0u8; 4];
    r.read_exact(&mut b).map_err(io)?;
    Ok(u32::from_le_bytes(b))
}

fn u64(r: &mut R) -> Result<u64, String> {
    let mut b = [0u8; 8];
    r.read_exact(&mut b).map_err(io)?;
    Ok(u64::from_le_bytes(b))
}

fn skip(r: &mut R, bytes: u64) -> Result<(), String> {
    let bytes = i64::try_from(bytes).map_err(|_| "cabeçalho GGUF inválido".to_owned())?;
    r.seek_relative(bytes).map_err(io)
}

/// A string, kept only when short enough to be useful.
fn string(r: &mut R) -> Result<Option<String>, String> {
    let len = u64(r)?;
    if len > MAX_STRING {
        // Skipped: not something we use.
        let pos = r.stream_position().map_err(io)?;
        r.seek(SeekFrom::Start(pos + len)).map_err(io)?;
        return Ok(None);
    }
    let mut buf = vec![0u8; len as usize];
    r.read_exact(&mut buf).map_err(io)?;
    Ok(Some(String::from_utf8_lossy(&buf).into_owned()))
}

fn scalar_size(kind: u32) -> Option<u64> {
    Some(match kind {
        0 | 1 | 7 => 1,
        2 | 3 => 2,
        4..=6 => 4,
        10..=12 => 8,
        _ => return None,
    })
}

fn value(r: &mut R, kind: u32, keep: bool) -> Result<Value, String> {
    match kind {
        8 => {
            if keep {
                Ok(string(r)?.map(Value::Text).unwrap_or(Value::Other))
            } else {
                let len = u64(r)?;
                skip(r, len)?;
                Ok(Value::Other)
            }
        }
        9 => {
            let elem = u32(r)?;
            let count = u64(r)?;
            if let Some(size) = scalar_size(elem) {
                skip(r, size.saturating_mul(count))?;
            } else if elem == 8 {
                for _ in 0..count {
                    let len = u64(r)?;
                    skip(r, len)?;
                }
            } else {
                return Err(format!(
                    "tipo de lista {elem} desconhecido no cabeçalho GGUF"
                ));
            }
            Ok(Value::Other)
        }
        _ => {
            let size = scalar_size(kind)
                .ok_or_else(|| format!("tipo {kind} desconhecido no cabeçalho GGUF"))?;
            let mut buf = [0u8; 8];
            r.read_exact(&mut buf[..size as usize]).map_err(io)?;
            Ok(match kind {
                0 | 2 | 4 | 10 => Value::Number(u64::from_le_bytes(buf)),
                1 | 3 | 5 | 11 => {
                    let raw = u64::from_le_bytes(buf);
                    let signed = match kind {
                        1 => raw as u8 as i8 as i64,
                        3 => raw as u16 as i16 as i64,
                        5 => raw as u32 as i32 as i64,
                        _ => raw as i64,
                    };
                    u64::try_from(signed)
                        .map(Value::Number)
                        .unwrap_or(Value::Other)
                }
                _ => Value::Other,
            })
        }
    }
}

/// `general.file_type` (llama.cpp's `llama_ftype`) as a name.
fn file_type(t: u64) -> Option<String> {
    let name = match t {
        0 => "F32",
        1 => "F16",
        2 => "Q4_0",
        3 => "Q4_1",
        7 => "Q8_0",
        8 => "Q5_0",
        9 => "Q5_1",
        10 => "Q2_K",
        11 => "Q3_K_S",
        12 => "Q3_K_M",
        13 => "Q3_K_L",
        14 => "Q4_K_S",
        15 => "Q4_K_M",
        16 => "Q5_K_S",
        17 => "Q5_K_M",
        18 => "Q6_K",
        19 => "IQ2_XXS",
        20 => "IQ2_XS",
        21 => "Q2_K_S",
        22 => "IQ3_XS",
        23 => "IQ3_XXS",
        24 => "IQ1_S",
        25 => "IQ4_NL",
        26 => "IQ3_S",
        27 => "IQ3_M",
        28 => "IQ2_S",
        29 => "IQ2_M",
        30 => "IQ4_XS",
        31 => "IQ1_M",
        32 => "BF16",
        36 => "TQ1_0",
        37 => "TQ2_0",
        38 => "MXFP4",
        _ => return None,
    };
    Some(name.to_owned())
}

/// Whether a chat template takes the tools the server passes: it reads the
/// `tools` variable (Qwen, Llama 3.1, Mistral, gpt-oss) or renders tool
/// calls (DeepSeek). Phi-4-mini's only reads `message['tools']`, which
/// llama.cpp never fills: its tools go by prompt instead.
pub fn template_takes_tools(template: &str) -> bool {
    let bytes = template.as_bytes();
    let name_char = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    let variable = template.match_indices("tools").any(|(i, _)| {
        let before = i.checked_sub(1).map(|j| bytes[j]);
        let after = bytes.get(i + "tools".len()).copied();
        // Not a quoted key, an attribute or part of a longer name.
        !before.is_some_and(|b| name_char(b) || matches!(b, b'\'' | b'"' | b'.'))
            && !after.is_some_and(|b| name_char(b) || matches!(b, b'\'' | b'"'))
    });
    variable || template.contains("tool_call")
}

/// Writes a minimal GGUF header (tests and tools that need a model file).
pub fn write_header(path: &Path, keys: &[(&str, HeaderValue)]) -> std::io::Result<()> {
    use std::io::Write;
    let mut out = Vec::new();
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&3u32.to_le_bytes());
    out.extend_from_slice(&0u64.to_le_bytes());
    out.extend_from_slice(&(keys.len() as u64).to_le_bytes());
    let put_str = |out: &mut Vec<u8>, s: &str| {
        out.extend_from_slice(&(s.len() as u64).to_le_bytes());
        out.extend_from_slice(s.as_bytes());
    };
    for (key, value) in keys {
        put_str(&mut out, key);
        match value {
            HeaderValue::U32(n) => {
                out.extend_from_slice(&4u32.to_le_bytes());
                out.extend_from_slice(&n.to_le_bytes());
            }
            HeaderValue::Str(s) => {
                out.extend_from_slice(&8u32.to_le_bytes());
                put_str(&mut out, s);
            }
            HeaderValue::Strings(list) => {
                out.extend_from_slice(&9u32.to_le_bytes());
                out.extend_from_slice(&8u32.to_le_bytes());
                out.extend_from_slice(&(list.len() as u64).to_le_bytes());
                for s in list {
                    put_str(&mut out, s);
                }
            }
            HeaderValue::F32s(list) => {
                out.extend_from_slice(&9u32.to_le_bytes());
                out.extend_from_slice(&6u32.to_le_bytes());
                out.extend_from_slice(&(list.len() as u64).to_le_bytes());
                for f in list {
                    out.extend_from_slice(&f.to_le_bytes());
                }
            }
        }
    }
    std::fs::File::create(path)?.write_all(&out)
}

/// A value for [`write_header`].
pub enum HeaderValue {
    U32(u32),
    Str(String),
    Strings(Vec<String>),
    F32s(Vec<f32>),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_what_a_model_says_about_itself() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("m.gguf");
        write_header(
            &path,
            &[
                ("general.architecture", HeaderValue::Str("qwen3".into())),
                ("general.name", HeaderValue::Str("Qwen3 8B".into())),
                ("general.size_label", HeaderValue::Str("8B".into())),
                ("general.file_type", HeaderValue::U32(15)),
                (
                    "tokenizer.ggml.tokens",
                    HeaderValue::Strings((0..5000).map(|i| format!("tok{i}")).collect()),
                ),
                ("tokenizer.ggml.scores", HeaderValue::F32s(vec![0.0; 5000])),
                ("qwen3.context_length", HeaderValue::U32(40960)),
                ("qwen3.block_count", HeaderValue::U32(36)),
                ("qwen3.embedding_length", HeaderValue::U32(4096)),
                ("qwen3.attention.head_count", HeaderValue::U32(32)),
                ("qwen3.attention.head_count_kv", HeaderValue::U32(8)),
                (
                    "tokenizer.chat_template",
                    HeaderValue::Str("{%- if tools %}…{%- endif %}".into()),
                ),
            ],
        )
        .unwrap();
        let info = read(&path).unwrap();
        assert_eq!(info.architecture.as_deref(), Some("qwen3"));
        assert_eq!(info.name.as_deref(), Some("Qwen3 8B"));
        assert_eq!(info.quantization.as_deref(), Some("Q4_K_M"));
        assert_eq!(info.context_length, Some(40960));
        assert!(info.takes_tools());
        // 36 layers × 8 KV heads × (128 + 128) × 2 bytes.
        assert_eq!(info.kv_bytes_per_token(), Some(36 * 8 * 256 * 2));
    }

    #[test]
    fn tells_which_templates_take_tools() {
        // Excerpts of the real templates.
        let qwen3 = "{%- if tools %}\n{{- '<|im_start|>system\\n' }}{%- for tool in tools %}";
        let llama31 = "{%- if not tools is defined %}\n{%- set tools = none %}\n{%- endif %}";
        let gpt_oss = "{%- if tools -%}{{- render_tool_namespace(\"functions\", tools) }}";
        let deepseek = "{%- for tool in message['tool_calls'] %}";
        let phi4_mini = "{% for message in messages %}{% if message['role'] == 'system' and 'tools' in message \
            and message['tools'] is not none %}{{ '<|' + message['role'] + '|>' + message['content'] + '<|tool|>' \
            + message['tools'] + '<|/tool|>' + '<|end|>' }}{% endif %}{% endfor %}";
        let gemma3 = "{{ bos_token }}{%- for message in loop_messages -%}<start_of_turn>{{ role }}";
        for t in [qwen3, llama31, gpt_oss, deepseek] {
            assert!(template_takes_tools(t), "{t}");
        }
        for t in [phi4_mini, gemma3, "{{ message.tools }}", "{{ toolset }}"] {
            assert!(!template_takes_tools(t), "{t}");
        }
    }

    #[test]
    fn refuses_what_is_not_gguf() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x.gguf");
        std::fs::write(&path, b"not a model at all").unwrap();
        assert!(read(&path).unwrap_err().contains("não é um modelo GGUF"));
        std::fs::write(&path, b"GG").unwrap();
        assert!(read(&path).unwrap_err().contains("curto"));
    }
}
