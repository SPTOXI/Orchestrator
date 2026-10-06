//! Downloads that survive interruptions (ADR-0025): written to
//! `<file>.part`, continued with `Range` when the server allows, checked
//! against the size and SHA-256 the source publishes, and only then moved
//! to the final name.

use futures_util::StreamExt;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tokio::io::AsyncWriteExt;
use tokio_util::sync::CancellationToken;

/// How often progress is reported, at most.
const PROGRESS_EVERY: Duration = Duration::from_millis(250);
/// No byte for this long: the download is dead.
const STALL: Duration = Duration::from_secs(120);

/// What the source says the file is.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Expected {
    pub size: Option<u64>,
    /// Hex SHA-256, lowercase.
    pub sha256: Option<String>,
}

/// Bytes so far and the total, when known.
pub type OnProgress<'a> = &'a (dyn Fn(u64, Option<u64>) + Send + Sync);

pub fn part_path(dest: &Path) -> PathBuf {
    let mut name = dest
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();
    name.push(".part");
    dest.with_file_name(name)
}

/// Downloads `url` to `dest`. Returns the file's SHA-256.
pub async fn fetch(
    client: &reqwest::Client,
    url: &str,
    dest: &Path,
    expected: &Expected,
    on_progress: OnProgress<'_>,
    cancel: &CancellationToken,
) -> Result<String, String> {
    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| format!("não foi possível criar {}: {e}", parent.display()))?;
    }
    let part = part_path(dest);
    let mut hasher = Sha256::new();
    // What a previous attempt left: hashed again, then continued.
    let mut have = match tokio::fs::metadata(&part).await {
        Ok(meta) if meta.len() > 0 && expected.size.is_none_or(|s| meta.len() < s) => {
            hash_file(&part, &mut hasher).await?;
            meta.len()
        }
        Ok(_) => {
            let _ = tokio::fs::remove_file(&part).await;
            0
        }
        Err(_) => 0,
    };
    let mut request = client.get(url);
    if have > 0 {
        request = request.header(reqwest::header::RANGE, format!("bytes={have}-"));
    }
    let response = tokio::select! {
        r = request.send() => r.map_err(|e| format!("falha ao baixar {}: {e}", short(url)))?,
        _ = cancel.cancelled() => return Err(cancelled()),
    };
    let status = response.status();
    if have > 0 && status == reqwest::StatusCode::OK {
        // The server ignored the range: start over.
        have = 0;
        hasher = Sha256::new();
    } else if !(status.is_success() || status == reqwest::StatusCode::PARTIAL_CONTENT) {
        return Err(format!(
            "falha ao baixar {}: HTTP {}",
            short(url),
            status.as_u16()
        ));
    }
    let total = expected
        .size
        .or_else(|| response.content_length().map(|len| len + have));
    let mut file = if have > 0 {
        tokio::fs::OpenOptions::new().append(true).open(&part).await
    } else {
        tokio::fs::File::create(&part).await
    }
    .map_err(|e| format!("não foi possível gravar {}: {e}", part.display()))?;
    let mut stream = response.bytes_stream();
    let mut last = Instant::now() - PROGRESS_EVERY;
    on_progress(have, total);
    loop {
        let chunk = tokio::select! {
            c = tokio::time::timeout(STALL, stream.next()) => match c {
                Ok(c) => c,
                Err(_) => return Err(format!("o download de {} parou de receber dados", short(url))),
            },
            _ = cancel.cancelled() => {
                let _ = file.flush().await;
                return Err(cancelled());
            }
        };
        let Some(chunk) = chunk else { break };
        let chunk = chunk.map_err(|e| format!("falha ao baixar {}: {e}", short(url)))?;
        hasher.update(&chunk);
        file.write_all(&chunk)
            .await
            .map_err(|e| format!("não foi possível gravar {}: {e}", part.display()))?;
        have += chunk.len() as u64;
        if last.elapsed() >= PROGRESS_EVERY {
            last = Instant::now();
            on_progress(have, total);
        }
    }
    file.flush().await.map_err(|e| e.to_string())?;
    drop(file);
    on_progress(have, total);
    let sha256 = hex::encode(hasher.finalize());
    let bad = |why: String| async {
        let _ = tokio::fs::remove_file(&part).await;
        Err(why)
    };
    if let Some(size) = expected.size {
        if have != size {
            return bad(format!(
                "o arquivo {} veio com {have} bytes, mas deveria ter {size}",
                short(url)
            ))
            .await;
        }
    }
    if let Some(want) = &expected.sha256 {
        if !want.eq_ignore_ascii_case(&sha256) {
            return bad(format!(
                "o arquivo {} não confere (SHA-256 diferente do publicado): baixe de novo",
                short(url)
            ))
            .await;
        }
    }
    tokio::fs::rename(&part, dest)
        .await
        .map_err(|e| format!("não foi possível mover {}: {e}", dest.display()))?;
    Ok(sha256)
}

async fn hash_file(path: &Path, hasher: &mut Sha256) -> Result<(), String> {
    use tokio::io::AsyncReadExt;
    let mut file = tokio::fs::File::open(path)
        .await
        .map_err(|e| e.to_string())?;
    let mut buf = vec![0u8; 1024 * 1024];
    loop {
        let n = file.read(&mut buf).await.map_err(|e| e.to_string())?;
        if n == 0 {
            return Ok(());
        }
        hasher.update(&buf[..n]);
    }
}

/// SHA-256 of a file on disk (hex).
pub async fn sha256_of(path: &Path) -> Result<String, String> {
    let mut hasher = Sha256::new();
    hash_file(path, &mut hasher).await?;
    Ok(hex::encode(hasher.finalize()))
}

pub fn cancelled() -> String {
    "download cancelado".into()
}

/// The file name of a URL, for messages.
fn short(url: &str) -> &str {
    url.split('?')
        .next()
        .unwrap_or(url)
        .rsplit('/')
        .next()
        .unwrap_or(url)
}
