//! Where engines and models come from (ADR-0025): the llama.cpp releases on
//! GitHub, Hugging Face repositories, the catalog, and the models Ollama
//! already downloaded.

use serde::Serialize;
use serde_json::Value;
use std::path::{Path, PathBuf};

/// Default addresses; tests point them at local servers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Urls {
    /// GitHub's REST API.
    pub github_api: String,
    /// The llama.cpp repository (`owner/repo`).
    pub engine_repo: String,
    pub huggingface: String,
}

impl Default for Urls {
    /// GitHub and Hugging Face, or mirrors: `ORCHESTRATOR_ENGINE_API` (a
    /// GitHub-compatible API with the llama.cpp releases) and `HF_ENDPOINT`
    /// (the variable Hugging Face's own tools read).
    fn default() -> Self {
        let env = |name: &str| {
            std::env::var(name)
                .ok()
                .map(|v| v.trim().trim_end_matches('/').to_owned())
                .filter(|v| !v.is_empty())
        };
        Self {
            github_api: env("ORCHESTRATOR_ENGINE_API")
                .unwrap_or_else(|| "https://api.github.com".into()),
            engine_repo: "ggml-org/llama.cpp".into(),
            huggingface: env("HF_ENDPOINT").unwrap_or_else(|| "https://huggingface.co".into()),
        }
    }
}

// ---------------------------------------------------------------- GitHub ---

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    pub tag: String,
    pub assets: Vec<ReleaseAsset>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseAsset {
    pub name: String,
    pub size: u64,
    pub url: String,
    /// Hex SHA-256 that GitHub computed (`digest: "sha256:…"`).
    pub sha256: Option<String>,
}

/// The llama.cpp releases, newest first, drafts left out. Not
/// `releases/latest`: llama.cpp marks every release as a pre-release, and
/// that endpoint skips pre-releases.
pub async fn releases(client: &reqwest::Client, urls: &Urls) -> Result<Vec<Release>, String> {
    let url = format!(
        "{}/repos/{}/releases?per_page=10",
        urls.github_api.trim_end_matches('/'),
        urls.engine_repo
    );
    let response = client
        .get(&url)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| format!("não foi possível consultar os releases do llama.cpp: {e}"))?;
    let status = response.status();
    if status == reqwest::StatusCode::FORBIDDEN || status == reqwest::StatusCode::TOO_MANY_REQUESTS
    {
        return Err(
            "o GitHub limitou as consultas deste computador: tente de novo em alguns minutos"
                .into(),
        );
    }
    if !status.is_success() {
        return Err(format!(
            "o GitHub respondeu HTTP {} ao consultar o llama.cpp",
            status.as_u16()
        ));
    }
    let body: Value = response
        .json()
        .await
        .map_err(|e| format!("resposta inválida do GitHub: {e}"))?;
    parse_releases(&body)
}

pub fn parse_releases(body: &Value) -> Result<Vec<Release>, String> {
    let list = body
        .as_array()
        .ok_or("resposta inválida do GitHub: esperava a lista de releases")?;
    let releases: Vec<Release> = list
        .iter()
        .filter(|r| !r["draft"].as_bool().unwrap_or(false))
        .filter_map(|r| parse_release(r).ok())
        .collect();
    if releases.is_empty() {
        return Err("o GitHub não listou nenhum release do llama.cpp".into());
    }
    Ok(releases)
}

pub fn parse_release(body: &Value) -> Result<Release, String> {
    let tag = body["tag_name"]
        .as_str()
        .ok_or("resposta do GitHub sem a versão (tag_name)")?
        .to_owned();
    let assets = body["assets"]
        .as_array()
        .map(|list| {
            list.iter()
                .filter_map(|a| {
                    Some(ReleaseAsset {
                        name: a["name"].as_str()?.to_owned(),
                        size: a["size"].as_u64().unwrap_or(0),
                        url: a["browser_download_url"].as_str()?.to_owned(),
                        sha256: a["digest"]
                            .as_str()
                            .and_then(|d| d.strip_prefix("sha256:"))
                            .map(str::to_ascii_lowercase),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(Release { tag, assets })
}

// ----------------------------------------------------------- Hugging Face ---

/// A `.gguf` file of a repository.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HfFile {
    pub path: String,
    pub size: u64,
    /// SHA-256 of the file (Git LFS object id).
    pub sha256: Option<String>,
}

impl HfFile {
    pub fn name(&self) -> &str {
        self.path.rsplit('/').next().unwrap_or(&self.path)
    }
}

/// `owner/repo`, from what the user typed (a URL works too).
pub fn repo_id(text: &str) -> Result<String, String> {
    let t = text.trim().trim_end_matches('/');
    let t = t
        .strip_prefix("https://huggingface.co/")
        .or_else(|| t.strip_prefix("http://huggingface.co/"))
        .or_else(|| t.strip_prefix("huggingface.co/"))
        .unwrap_or(t);
    let mut parts = t.split('/');
    let (Some(owner), Some(repo)) = (parts.next(), parts.next()) else {
        return Err("informe o repositório como dono/nome (ex.: Qwen/Qwen3-8B-GGUF)".into());
    };
    let ok = |s: &str| {
        !s.is_empty()
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
    };
    if !ok(owner) || !ok(repo) {
        return Err("repositório inválido: use dono/nome (ex.: Qwen/Qwen3-8B-GGUF)".into());
    }
    Ok(format!("{owner}/{repo}"))
}

/// The `.gguf` files of a repository (projectors for images left out).
pub async fn hf_files(
    client: &reqwest::Client,
    urls: &Urls,
    repo: &str,
) -> Result<Vec<HfFile>, String> {
    let url = format!(
        "{}/api/models/{repo}/tree/main?recursive=true",
        urls.huggingface.trim_end_matches('/')
    );
    let response = client
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("não foi possível consultar o Hugging Face: {e}"))?;
    match response.status().as_u16() {
        200 => {}
        401 | 403 => return Err(format!("{repo} é privado ou restrito no Hugging Face")),
        404 => return Err(format!("o repositório {repo} não existe no Hugging Face")),
        code => return Err(format!("o Hugging Face respondeu HTTP {code}")),
    }
    let body: Value = response
        .json()
        .await
        .map_err(|e| format!("resposta inválida do Hugging Face: {e}"))?;
    let mut files: Vec<HfFile> = body
        .as_array()
        .map(|list| {
            list.iter()
                .filter(|f| f["type"] == "file")
                .filter_map(|f| {
                    let path = f["path"].as_str()?.to_owned();
                    let lower = path.to_ascii_lowercase();
                    (lower.ends_with(".gguf") && !lower.rsplit('/').next()?.starts_with("mmproj"))
                        .then(|| HfFile {
                            size: f["lfs"]["size"]
                                .as_u64()
                                .or_else(|| f["size"].as_u64())
                                .unwrap_or(0),
                            sha256: f["lfs"]["oid"].as_str().map(str::to_ascii_lowercase),
                            path,
                        })
                })
                .collect()
        })
        .unwrap_or_default();
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(files)
}

pub fn hf_url(urls: &Urls, repo: &str, path: &str) -> String {
    format!(
        "{}/{repo}/resolve/main/{path}",
        urls.huggingface.trim_end_matches('/')
    )
}

/// `-00001-of-00003`: the part number and the count.
fn split_part(name: &str) -> Option<(u32, u32, String)> {
    let stem = name.strip_suffix(".gguf")?;
    let (head, of) = stem.rsplit_once("-of-")?;
    let (base, part) = head.rsplit_once('-')?;
    if part.len() != 5 || of.len() != 5 {
        return None;
    }
    Some((part.parse().ok()?, of.parse().ok()?, base.to_owned()))
}

/// The files that make up `chosen`: itself, or every part of a model split
/// in parts (first part first).
pub fn parts_of(files: &[HfFile], chosen: &str) -> Vec<HfFile> {
    let Some(file) = files.iter().find(|f| f.path == chosen) else {
        return Vec::new();
    };
    let dir = |p: &str| {
        p.rsplit_once('/')
            .map(|(d, _)| d.to_owned())
            .unwrap_or_default()
    };
    match split_part(file.name()) {
        Some((_, count, base)) => {
            let mut parts: Vec<(u32, HfFile)> = files
                .iter()
                .filter(|f| dir(&f.path) == dir(&file.path))
                .filter_map(|f| {
                    let (n, c, b) = split_part(f.name())?;
                    (c == count && b == base).then(|| (n, f.clone()))
                })
                .collect();
            parts.sort_by_key(|(n, _)| *n);
            if parts.len() as u32 != count {
                return Vec::new();
            }
            parts.into_iter().map(|(_, f)| f).collect()
        }
        None => vec![file.clone()],
    }
}

/// The file of `quant` ("Q4_K_M") in a repository: the first part, when
/// the model comes in parts.
pub fn pick_quant(files: &[HfFile], quant: &str) -> Option<String> {
    let quant = quant.to_ascii_lowercase();
    let matches = |f: &HfFile| {
        let name = f.name().to_ascii_lowercase();
        let stem = match split_part(&name) {
            Some((_, _, base)) => base,
            None => name.strip_suffix(".gguf").unwrap_or(&name).to_owned(),
        };
        stem.strip_suffix(&quant)
            .is_some_and(|before| before.ends_with(['-', '.', '_']))
    };
    let mut found: Vec<&HfFile> = files.iter().filter(|f| matches(f)).collect();
    // A single file before a model in parts; the root before folders.
    found.sort_by_key(|f| {
        (
            split_part(f.name()).is_some(),
            f.path.matches('/').count(),
            f.path.clone(),
        )
    });
    let first = found.first()?;
    match split_part(first.name()) {
        Some(_) => parts_of(files, &first.path).first().map(|f| f.path.clone()),
        None => Some(first.path.clone()),
    }
}

// --------------------------------------------------------------- catalog ---

/// A model the catalog suggests (ADR-0025): a Hugging Face repository and
/// a quantization.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogModel {
    pub id: &'static str,
    pub label: &'static str,
    pub repo: &'static str,
    pub quant: &'static str,
    /// Download size, roughly (GB).
    pub size_gb: f32,
    /// Memory to run it well (RAM or graphics memory), roughly (GB).
    pub memory_gb: u32,
    pub tools: bool,
    pub note: &'static str,
}

pub fn catalog() -> Vec<CatalogModel> {
    vec![
        CatalogModel {
            id: "qwen3-8b",
            label: "Qwen3 8B",
            repo: "Qwen/Qwen3-8B-GGUF",
            quant: "Q4_K_M",
            size_gb: 5.0,
            memory_gb: 8,
            tools: true,
            note: "bom equilíbrio para código e conversa; pensa antes de responder",
        },
        CatalogModel {
            id: "qwen2.5-coder-7b",
            label: "Qwen2.5 Coder 7B",
            repo: "Qwen/Qwen2.5-Coder-7B-Instruct-GGUF",
            quant: "q4_k_m",
            size_gb: 4.7,
            memory_gb: 8,
            tools: true,
            note: "focado em código",
        },
        CatalogModel {
            id: "qwen3-4b",
            label: "Qwen3 4B",
            repo: "Qwen/Qwen3-4B-GGUF",
            quant: "Q4_K_M",
            size_gb: 2.5,
            memory_gb: 5,
            tools: true,
            note: "para computadores mais modestos",
        },
        CatalogModel {
            id: "llama3.1-8b",
            label: "Llama 3.1 8B",
            repo: "bartowski/Meta-Llama-3.1-8B-Instruct-GGUF",
            quant: "Q4_K_M",
            size_gb: 4.9,
            memory_gb: 8,
            tools: true,
            note: "uso geral",
        },
        CatalogModel {
            id: "qwen2.5-coder-14b",
            label: "Qwen2.5 Coder 14B",
            repo: "Qwen/Qwen2.5-Coder-14B-Instruct-GGUF",
            quant: "q4_k_m",
            size_gb: 9.0,
            memory_gb: 16,
            tools: true,
            note: "código, melhor que o 7B; pede mais memória",
        },
        CatalogModel {
            id: "gpt-oss-20b",
            label: "gpt-oss 20B (OpenAI)",
            repo: "ggml-org/gpt-oss-20b-GGUF",
            quant: "mxfp4",
            size_gb: 12.1,
            memory_gb: 16,
            tools: true,
            note: "raciocínio forte; para máquinas com 16 GB ou mais",
        },
        CatalogModel {
            id: "gemma3-4b",
            label: "Gemma 3 4B",
            repo: "ggml-org/gemma-3-4b-it-GGUF",
            quant: "Q4_K_M",
            size_gb: 2.5,
            memory_gb: 5,
            tools: false,
            note: "leve; as ferramentas vão por prompt",
        },
    ]
}

// ---------------------------------------------------------------- Ollama ---

const MODEL_LAYER: &str = "application/vnd.ollama.image.model";

/// A model Ollama downloaded: its GGUF is a file in Ollama's `blobs`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OllamaModel {
    /// As Ollama shows it: `phi4-mini:latest`.
    pub name: String,
    pub blob: PathBuf,
    pub size: u64,
    /// Hex SHA-256 (the blob's name).
    pub sha256: String,
}

/// Where Ollama keeps its models on this computer.
pub fn ollama_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(custom) = std::env::var_os("OLLAMA_MODELS").filter(|v| !v.is_empty()) {
        dirs.push(PathBuf::from(custom));
    }
    let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" });
    if let Some(home) = home {
        dirs.push(PathBuf::from(home).join(".ollama").join("models"));
    }
    if cfg!(target_os = "linux") {
        dirs.push(PathBuf::from("/usr/share/ollama/.ollama/models"));
        dirs.push(PathBuf::from("/var/lib/ollama/.ollama/models"));
    }
    dirs.dedup();
    dirs
}

/// The models in Ollama's folders (`manifests/<host>/<namespace>/<name>/<tag>`).
pub fn ollama_models(dirs: &[PathBuf]) -> Vec<OllamaModel> {
    let mut found: Vec<OllamaModel> = Vec::new();
    for dir in dirs {
        let manifests = dir.join("manifests");
        let Ok(hosts) = std::fs::read_dir(&manifests) else {
            continue;
        };
        for host in hosts.flatten() {
            for namespace in read_dirs(&host.path()) {
                for model in read_dirs(&namespace) {
                    let Ok(tags) = std::fs::read_dir(&model) else {
                        continue;
                    };
                    for tag in tags.flatten() {
                        let path = tag.path();
                        if !path.is_file() {
                            continue;
                        }
                        if let Some(m) =
                            manifest_model(dir, &host.path(), &namespace, &model, &path)
                        {
                            if !found.iter().any(|f| f.name == m.name) {
                                found.push(m);
                            }
                        }
                    }
                }
            }
        }
    }
    found.sort_by(|a, b| a.name.cmp(&b.name));
    found
}

fn read_dirs(path: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(path)
        .map(|entries| {
            entries
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.is_dir())
                .collect()
        })
        .unwrap_or_default()
}

fn manifest_model(
    root: &Path,
    host: &Path,
    namespace: &Path,
    model: &Path,
    file: &Path,
) -> Option<OllamaModel> {
    let text = std::fs::read_to_string(file).ok()?;
    let manifest: Value = serde_json::from_str(&text).ok()?;
    let layer = manifest["layers"]
        .as_array()?
        .iter()
        .find(|l| l["mediaType"] == MODEL_LAYER)?;
    let digest = layer["digest"].as_str()?;
    let hex = digest
        .strip_prefix("sha256:")
        .or_else(|| digest.strip_prefix("sha256-"))?;
    let blobs = root.join("blobs");
    let blob = [format!("sha256-{hex}"), format!("sha256:{hex}")]
        .iter()
        .map(|n| blobs.join(n))
        .find(|p| p.is_file())?;
    let size = std::fs::metadata(&blob).ok()?.len();
    let name_of = |p: &Path| p.file_name()?.to_str().map(str::to_owned);
    let (host, namespace, model, tag) = (
        name_of(host)?,
        name_of(namespace)?,
        name_of(model)?,
        name_of(file)?,
    );
    let name = if host == "registry.ollama.ai" && namespace == "library" {
        format!("{model}:{tag}")
    } else if host == "registry.ollama.ai" {
        format!("{namespace}/{model}:{tag}")
    } else {
        format!("{host}/{namespace}/{model}:{tag}")
    };
    Some(OllamaModel {
        name,
        blob,
        size,
        sha256: hex.to_ascii_lowercase(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn file(path: &str) -> HfFile {
        HfFile {
            path: path.into(),
            size: 1,
            sha256: None,
        }
    }

    #[test]
    fn picks_the_quantization_and_its_parts() {
        let files = vec![
            file("Qwen3-8B-Q4_K_M.gguf"),
            file("Qwen3-8B-Q5_K_M.gguf"),
            file("Qwen3-8B-Q8_0.gguf"),
            file("big/model-q4_k_m-00002-of-00002.gguf"),
            file("big/model-q4_k_m-00001-of-00002.gguf"),
            file("mmproj-F16.gguf"),
        ];
        assert_eq!(
            pick_quant(&files, "Q4_K_M").as_deref(),
            Some("Qwen3-8B-Q4_K_M.gguf")
        );
        assert_eq!(
            pick_quant(&files, "q8_0").as_deref(),
            Some("Qwen3-8B-Q8_0.gguf")
        );
        assert_eq!(pick_quant(&files, "Q4_K"), None);
        let split = vec![
            file("model-q4_k_m-00002-of-00002.gguf"),
            file("model-q4_k_m-00001-of-00002.gguf"),
        ];
        assert_eq!(
            pick_quant(&split, "q4_k_m").as_deref(),
            Some("model-q4_k_m-00001-of-00002.gguf")
        );
        let parts = parts_of(&files, "big/model-q4_k_m-00002-of-00002.gguf");
        assert_eq!(
            parts.iter().map(|p| p.path.as_str()).collect::<Vec<_>>(),
            [
                "big/model-q4_k_m-00001-of-00002.gguf",
                "big/model-q4_k_m-00002-of-00002.gguf"
            ]
        );
        // A missing part: nothing to download.
        assert!(parts_of(&split[..1], "model-q4_k_m-00002-of-00002.gguf").is_empty());
    }

    #[test]
    fn reads_repository_names_and_release_files() {
        assert_eq!(
            repo_id(" https://huggingface.co/Qwen/Qwen3-8B-GGUF/ ").unwrap(),
            "Qwen/Qwen3-8B-GGUF"
        );
        assert_eq!(
            repo_id("bartowski/Meta-Llama-3.1-8B-Instruct-GGUF").unwrap(),
            "bartowski/Meta-Llama-3.1-8B-Instruct-GGUF"
        );
        assert!(repo_id("só-um-nome").is_err());
        assert!(repo_id("a/b c").is_err());
        let release = parse_release(&json!({
            "tag_name": "b9000",
            "assets": [{"name": "x.zip", "size": 3, "browser_download_url": "https://h/x.zip", "digest": "sha256:ABC"}]
        }))
        .unwrap();
        assert_eq!(release.tag, "b9000");
        assert_eq!(release.assets[0].sha256.as_deref(), Some("abc"));
        // llama.cpp's are all pre-releases: they count; drafts do not.
        let list = parse_releases(&json!([
            {"tag_name": "b9200", "draft": true, "prerelease": true, "assets": []},
            {"tag_name": "b9100", "draft": false, "prerelease": true, "assets": []},
            {"tag_name": "b9000", "draft": false, "prerelease": false, "assets": []}
        ]))
        .unwrap();
        let tags: Vec<&str> = list.iter().map(|r| r.tag.as_str()).collect();
        assert_eq!(tags, ["b9100", "b9000"]);
        assert!(parse_releases(&json!([])).is_err());
        assert!(parse_releases(&json!({"message": "Not Found"})).is_err());
    }

    #[test]
    fn finds_the_models_ollama_downloaded() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("models");
        let manifest = root.join("manifests/registry.ollama.ai/library/phi4-mini/latest");
        std::fs::create_dir_all(manifest.parent().unwrap()).unwrap();
        std::fs::create_dir_all(root.join("blobs")).unwrap();
        std::fs::write(root.join("blobs/sha256-aaaa"), b"GGUF-model").unwrap();
        std::fs::write(
            &manifest,
            json!({"layers": [
                {"mediaType": "application/vnd.ollama.image.template", "digest": "sha256:bbbb"},
                {"mediaType": "application/vnd.ollama.image.model", "digest": "sha256:aaaa", "size": 10}
            ]})
            .to_string(),
        )
        .unwrap();
        let other = root.join("manifests/hf.co/bartowski/Qwen3-GGUF/Q4_K_M");
        std::fs::create_dir_all(other.parent().unwrap()).unwrap();
        std::fs::write(&other, json!({"layers": [{"mediaType": "application/vnd.ollama.image.model", "digest": "sha256:aaaa"}]}).to_string()).unwrap();
        let models = ollama_models(&[root.clone(), dir.path().join("missing")]);
        assert_eq!(
            models.iter().map(|m| m.name.as_str()).collect::<Vec<_>>(),
            ["hf.co/bartowski/Qwen3-GGUF:Q4_K_M", "phi4-mini:latest"]
        );
        assert_eq!(models[1].blob, root.join("blobs/sha256-aaaa"));
        assert_eq!(models[1].size, 10);
        assert_eq!(models[1].sha256, "aaaa");
    }
}
