use std::path::PathBuf;

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("could not read {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("not a .nativa file (wrong magic)")]
    BadMagic,
    #[error(".nativa format {found} is newer than this engine (reads up to {supported})")]
    UnsupportedVersion { found: u32, supported: u32 },
    #[error("invalid .nativa header: {0}")]
    BadHeader(String),
    #[error("tensor {name}: {reason}")]
    BadTensor { name: String, reason: String },
    #[error("missing tensor {0}")]
    MissingTensor(String),
    #[error("invalid model config: {0}")]
    BadConfig(String),
    #[error("needs {needed} bytes of memory, over the budget of {budget}")]
    OverBudget { needed: usize, budget: usize },
    #[error("token {token} is outside the vocabulary ({vocab_size})")]
    BadToken { token: u32, vocab_size: usize },
}
