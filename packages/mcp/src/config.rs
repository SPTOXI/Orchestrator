//! `mcp.json`: the MCP servers the user added. Also reads the format of
//! Claude Desktop, Claude Code and Cursor (`{"mcpServers": {...}}`) to
//! import them.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// Default time an MCP tool call may take.
pub const DEFAULT_TIMEOUT_SECS: u64 = 300;

/// How the Orchestrator talks to a server.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum Transport {
    /// A program the Orchestrator starts; JSON-RPC over its stdin/stdout.
    #[default]
    Stdio,
    /// Streamable HTTP: JSON-RPC posted to a URL.
    Http,
}

/// One server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerConfig {
    /// Lowercase letters, digits and `-`; part of the tool names
    /// (`mcp.<id>.<tool>`).
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub transport: Transport,
    #[serde(default)]
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    /// Values may use `{{secret:NAME}}`.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub url: String,
    /// Values may use `{{secret:NAME}}`.
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    #[serde(default = "enabled")]
    pub enabled: bool,
    /// Seconds a tool call may take (default 300).
    #[serde(default)]
    pub timeout_secs: Option<u64>,
    /// Tools the AIs do not get, by the server's name for them.
    #[serde(default)]
    pub disabled_tools: Vec<String>,
}

fn enabled() -> bool {
    true
}

impl ServerConfig {
    pub fn label(&self) -> &str {
        if self.name.trim().is_empty() {
            &self.id
        } else {
            &self.name
        }
    }

    pub fn timeout(&self) -> std::time::Duration {
        std::time::Duration::from_secs(self.timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS).max(1))
    }

    pub fn validate(&self) -> Result<(), String> {
        if !valid_id(&self.id) {
            return Err("id: letras minúsculas, números e - (até 24, começando por letra)".into());
        }
        match self.transport {
            Transport::Stdio if self.command.trim().is_empty() => {
                Err("informe o comando que inicia o servidor".into())
            }
            Transport::Http
                if !(self.url.starts_with("http://") || self.url.starts_with("https://")) =>
            {
                Err("a URL do servidor começa com http:// ou https://".into())
            }
            _ => Ok(()),
        }
    }
}

pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 24
        && id.starts_with(|c: char| c.is_ascii_lowercase())
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// An id from a free name: `GitHub Oficial` → `github-oficial`.
pub fn id_from(name: &str) -> String {
    let mut id = String::new();
    for c in name.trim().to_lowercase().chars() {
        let c = match c {
            'á' | 'à' | 'â' | 'ã' | 'ä' => 'a',
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'í' | 'ì' | 'î' | 'ï' => 'i',
            'ó' | 'ò' | 'ô' | 'õ' | 'ö' => 'o',
            'ú' | 'ù' | 'û' | 'ü' => 'u',
            'ç' => 'c',
            c => c,
        };
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            id.push(c);
        } else if !id.ends_with('-') && !id.is_empty() {
            id.push('-');
        }
    }
    let mut id: String = id.trim_matches('-').chars().take(24).collect();
    id = id.trim_end_matches('-').to_owned();
    if !id.starts_with(|c: char| c.is_ascii_lowercase()) {
        id.insert_str(0, "mcp-");
        id.truncate(24);
    }
    id
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpFile {
    #[serde(default)]
    pub servers: Vec<ServerConfig>,
}

/// Servers from JSON in Claude Desktop / Claude Code / Cursor format
/// (`{"mcpServers": {"name": {"command", "args", "env"} | {"type": "http",
/// "url", "headers"}}}`), or a bare map of servers.
pub fn import(text: &str) -> Result<Vec<ServerConfig>, String> {
    let value: Value =
        serde_json::from_str(text).map_err(|e| format!("não é um JSON válido: {e}"))?;
    let map = value
        .get("mcpServers")
        .or_else(|| value.get("servers"))
        .unwrap_or(&value)
        .as_object()
        .ok_or("esperava {\"mcpServers\": {\"nome\": {...}}}")?;
    let mut out = Vec::new();
    for (name, entry) in map {
        let text = |key: &str| {
            entry
                .get(key)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned()
        };
        let strings = |key: &str| -> BTreeMap<String, String> {
            entry
                .get(key)
                .and_then(Value::as_object)
                .map(|m| {
                    m.iter()
                        .map(|(k, v)| {
                            (
                                k.clone(),
                                v.as_str().map_or_else(|| v.to_string(), str::to_owned),
                            )
                        })
                        .collect()
                })
                .unwrap_or_default()
        };
        let url = {
            let url = text("url");
            if url.is_empty() {
                text("httpUrl")
            } else {
                url
            }
        };
        let kind = text("type");
        let transport = if !url.is_empty() && kind != "stdio" {
            Transport::Http
        } else {
            Transport::Stdio
        };
        let config = ServerConfig {
            id: id_from(name),
            name: name.clone(),
            transport,
            command: text("command"),
            args: entry
                .get("args")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default(),
            env: strings("env"),
            cwd: entry.get("cwd").and_then(Value::as_str).map(str::to_owned),
            url,
            headers: strings("headers"),
            enabled: !entry
                .get("disabled")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            timeout_secs: entry
                .get("timeout")
                .and_then(Value::as_u64)
                .map(|ms| (ms / 1000).max(1)),
            disabled_tools: Vec::new(),
        };
        config
            .validate()
            .map_err(|e| format!("servidor \"{name}\": {e}"))?;
        out.push(config);
    }
    if out.is_empty() {
        return Err("nenhum servidor no JSON".into());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_and_imports() {
        assert_eq!(id_from("GitHub Oficial"), "github-oficial");
        assert_eq!(id_from("  Ação!! 2 "), "acao-2");
        assert_eq!(id_from("123"), "mcp-123");
        assert!(valid_id(&id_from(
            "um nome bem comprido demais para caber aqui"
        )));

        let servers = import(
            r#"{"mcpServers": {
                "Playwright": {"command": "npx", "args": ["@playwright/mcp@latest"]},
                "github": {"type": "http", "url": "https://api.githubcopilot.com/mcp/",
                           "headers": {"Authorization": "Bearer {{secret:GITHUB_TOKEN}}"}},
                "off": {"command": "x", "disabled": true, "env": {"A": "1", "N": 2}}
            }}"#,
        )
        .unwrap();
        let by = |id: &str| servers.iter().find(|s| s.id == id).unwrap();
        assert_eq!(by("playwright").args, vec!["@playwright/mcp@latest"]);
        assert_eq!(by("playwright").name, "Playwright");
        assert_eq!(by("github").transport, Transport::Http);
        assert!(by("github").headers["Authorization"].contains("{{secret:GITHUB_TOKEN}}"));
        assert!(!by("off").enabled);
        assert_eq!(by("off").env["N"], "2");

        assert!(import("[]").is_err());
        assert!(
            import(r#"{"mcpServers": {"x": {}}}"#).is_err(),
            "no command"
        );
    }
}
