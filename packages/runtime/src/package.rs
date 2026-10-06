//! Package managers (`package.install`, `package.run`) and runtime probes
//! (`runtime.node`, `runtime.python`, `runtime.docker`), ADR-0008.
//!
//! Commands run through the user's shell (same path as `shell.execute`, so a
//! login shell's PATH applies). Every argument is quoted for that shell, so
//! package names and script arguments are never interpreted by it.

use crate::process::ProcessInfo;
use crate::shell::{self, ExecuteArgs, ExecuteOutput, ShellKind, ShellRegistry};
use orchestrator_core::{ToolError, ToolErrorKind};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

/// Managers `package.*` knows how to drive.
pub const SUPPORTED_MANAGERS: &[&str] = &[
    "pnpm", "yarn", "npm", "bun", "poetry", "uv", "pipenv", "pip", "cargo", "go",
];

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InstallArgs {
    /// Project folder (default: the open project).
    #[serde(default)]
    pub path: Option<String>,
    /// Packages to add; empty installs the project's dependencies.
    #[serde(default)]
    pub packages: Vec<String>,
    /// Add as development dependency.
    #[serde(default)]
    pub dev: bool,
    /// Override the detected manager.
    #[serde(default)]
    pub manager: Option<String>,
    /// Give up after this many milliseconds (no timeout when absent).
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunArgs {
    /// Script (npm/pnpm/yarn/bun), command (poetry/uv/pipenv run) or cargo/go
    /// subcommand (`test`, `build`, …).
    pub script: String,
    /// Extra arguments passed to the script.
    #[serde(default)]
    pub args: Vec<String>,
    /// Project folder (default: the open project).
    #[serde(default)]
    pub path: Option<String>,
    /// Override the detected package manager.
    #[serde(default)]
    pub manager: Option<String>,
    /// Start as a managed process (dev servers, watchers) instead of waiting.
    #[serde(default)]
    pub background: bool,
    /// Give up after this many milliseconds (no timeout when absent).
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageOutput {
    pub manager: String,
    pub command: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<ExecuteOutput>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub process: Option<ProcessInfo>,
}

/// Picks the manager: explicit choice, else the profile's first supported one.
pub fn choose_manager(
    requested: Option<&str>,
    detected: &[String],
    allowed: &[&str],
) -> Result<String, ToolError> {
    if let Some(requested) = requested {
        let requested = requested.trim().to_lowercase();
        if !SUPPORTED_MANAGERS.contains(&requested.as_str()) {
            return Err(ToolError::invalid_args(format!(
                "unsupported package manager: {requested} (supported: {})",
                SUPPORTED_MANAGERS.join(", ")
            )));
        }
        return Ok(requested);
    }
    detected
        .iter()
        .find(|m| allowed.contains(&m.as_str()))
        .cloned()
        .ok_or_else(|| {
            ToolError::new(
                ToolErrorKind::NotFound,
                "no supported package manager detected in this project (pass manager explicitly)",
            )
        })
}

fn python() -> &'static str {
    if cfg!(windows) {
        "python"
    } else {
        "python3"
    }
}

fn words(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| (*s).to_owned()).collect()
}

/// Command words for `package.install`.
pub fn install_command(
    manager: &str,
    packages: &[String],
    dev: bool,
    project: &Path,
) -> Result<Vec<String>, ToolError> {
    let adding = !packages.is_empty();
    let mut command = match (manager, adding) {
        ("npm", false) => words(&["npm", "install"]),
        ("npm", true) => words(if dev {
            &["npm", "install", "--save-dev"]
        } else {
            &["npm", "install"]
        }),
        ("pnpm", false) => words(&["pnpm", "install"]),
        ("pnpm", true) => words(if dev {
            &["pnpm", "add", "-D"]
        } else {
            &["pnpm", "add"]
        }),
        ("yarn", false) => words(&["yarn", "install"]),
        ("yarn", true) => words(if dev {
            &["yarn", "add", "--dev"]
        } else {
            &["yarn", "add"]
        }),
        ("bun", false) => words(&["bun", "install"]),
        ("bun", true) => words(if dev {
            &["bun", "add", "--dev"]
        } else {
            &["bun", "add"]
        }),
        ("poetry", false) => words(&["poetry", "install"]),
        ("poetry", true) => words(if dev {
            &["poetry", "add", "--group", "dev"]
        } else {
            &["poetry", "add"]
        }),
        ("uv", false) => words(&["uv", "sync"]),
        ("uv", true) => words(if dev {
            &["uv", "add", "--dev"]
        } else {
            &["uv", "add"]
        }),
        ("pipenv", _) => words(if dev {
            &["pipenv", "install", "--dev"]
        } else {
            &["pipenv", "install"]
        }),
        ("pip", false) => {
            if project.join("requirements.txt").is_file() {
                words(&[python(), "-m", "pip", "install", "-r", "requirements.txt"])
            } else {
                words(&[python(), "-m", "pip", "install", "."])
            }
        }
        ("pip", true) => words(&[python(), "-m", "pip", "install"]),
        ("cargo", false) => words(&["cargo", "fetch"]),
        ("cargo", true) => words(if dev {
            &["cargo", "add", "--dev"]
        } else {
            &["cargo", "add"]
        }),
        ("go", false) => words(&["go", "mod", "download"]),
        ("go", true) => words(&["go", "get"]),
        (other, _) => {
            return Err(ToolError::invalid_args(format!(
                "package.install does not support {other}"
            )))
        }
    };
    command.extend(packages.iter().cloned());
    Ok(command)
}

/// Command words for `package.run`.
pub fn run_command(manager: &str, script: &str, args: &[String]) -> Result<Vec<String>, ToolError> {
    if script.trim().is_empty() {
        return Err(ToolError::invalid_args("script must not be empty"));
    }
    let mut command = match manager {
        "npm" => words(&["npm", "run", script]),
        "pnpm" | "yarn" | "bun" => words(&[manager, "run", script]),
        "poetry" | "uv" | "pipenv" => words(&[manager, "run", script]),
        "cargo" | "go" => words(&[manager, script]),
        other => {
            return Err(ToolError::invalid_args(format!(
                "package.run does not support {other} (use shell.execute)"
            )))
        }
    };
    if !args.is_empty() {
        // npm needs `--` to forward arguments to the script.
        if manager == "npm" {
            command.push("--".into());
        }
        command.extend(args.iter().cloned());
    }
    Ok(command)
}

/// Quotes one argument for `kind` when it contains anything but plain
/// characters.
pub fn quote(kind: ShellKind, arg: &str) -> Result<String, ToolError> {
    let plain =
        |c: char| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '/' | ':' | '=' | '+');
    match kind {
        ShellKind::Pwsh | ShellKind::PowerShell => {
            // `@` starts splatting and `,` builds arrays in PowerShell.
            if !arg.is_empty() && arg.chars().all(plain) {
                Ok(arg.to_owned())
            } else {
                Ok(format!("'{}'", arg.replace('\'', "''")))
            }
        }
        ShellKind::Cmd => {
            if arg.contains('"') || arg.contains('%') {
                return Err(ToolError::invalid_args(format!(
                    "argument cannot be passed safely through cmd.exe: {arg}"
                )));
            }
            if !arg.is_empty() && arg.chars().all(|c| plain(c) || c == '@') {
                Ok(arg.to_owned())
            } else {
                Ok(format!("\"{arg}\""))
            }
        }
        _ => {
            if !arg.is_empty()
                && arg
                    .chars()
                    .all(|c| plain(c) || matches!(c, '@' | '%' | ','))
            {
                Ok(arg.to_owned())
            } else {
                Ok(format!("'{}'", arg.replace('\'', "'\\''")))
            }
        }
    }
}

pub fn render(kind: ShellKind, words: &[String]) -> Result<String, ToolError> {
    let quoted: Result<Vec<String>, ToolError> = words.iter().map(|w| quote(kind, w)).collect();
    Ok(quoted?.join(" "))
}

// --------------------------------------------------------------- probes ---

const PROBE_TIMEOUT_MS: u64 = 15_000;

/// Runs a version command in the default shell; returns the first line of
/// output when it succeeds.
pub async fn probe(registry: &ShellRegistry, cwd: &Path, words: &[&str]) -> Option<String> {
    let shell = registry.resolve(None).ok()?;
    let owned: Vec<String> = words.iter().map(|w| (*w).to_owned()).collect();
    let command = render(shell.kind, &owned).ok()?;
    let out = shell::execute(
        registry,
        cwd,
        ExecuteArgs {
            command,
            cwd: None,
            shell: None,
            env: Default::default(),
            timeout_ms: Some(PROBE_TIMEOUT_MS),
            stdin: None,
            max_output_bytes: Some(64 * 1024),
        },
        None,
    )
    .await
    .ok()?;
    if out.exit_code != Some(0) {
        return None;
    }
    // Some tools (old pythons) print the version on stderr.
    [out.stdout, out.stderr]
        .iter()
        .flat_map(|text| text.lines())
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(str::to_owned)
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeRuntime {
    pub available: bool,
    pub version: Option<String>,
    /// npm, pnpm, yarn and bun versions (`null` when not installed).
    pub managers: BTreeMap<String, Option<String>>,
}

pub async fn node(registry: &ShellRegistry, cwd: &Path) -> NodeRuntime {
    let (node, npm, pnpm, yarn, bun) = tokio::join!(
        probe(registry, cwd, &["node", "--version"]),
        probe(registry, cwd, &["npm", "--version"]),
        probe(registry, cwd, &["pnpm", "--version"]),
        probe(registry, cwd, &["yarn", "--version"]),
        probe(registry, cwd, &["bun", "--version"]),
    );
    NodeRuntime {
        available: node.is_some(),
        version: node,
        managers: BTreeMap::from([
            ("npm".to_owned(), npm),
            ("pnpm".to_owned(), pnpm),
            ("yarn".to_owned(), yarn),
            ("bun".to_owned(), bun),
        ]),
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PythonRuntime {
    pub available: bool,
    pub version: Option<String>,
    /// Interpreter command that worked (`python3`, `python`, `py`).
    pub command: Option<String>,
    pub pip: Option<String>,
}

pub async fn python_runtime(registry: &ShellRegistry, cwd: &Path) -> PythonRuntime {
    let candidates: &[&str] = if cfg!(windows) {
        &["python", "py", "python3"]
    } else {
        &["python3", "python"]
    };
    for candidate in candidates {
        if let Some(version) = probe(registry, cwd, &[candidate, "--version"]).await {
            let pip = probe(registry, cwd, &[candidate, "-m", "pip", "--version"]).await;
            return PythonRuntime {
                available: true,
                version: Some(version),
                command: Some((*candidate).to_owned()),
                pip,
            };
        }
    }
    PythonRuntime {
        available: false,
        version: None,
        command: None,
        pip: None,
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DockerRuntime {
    pub available: bool,
    pub version: Option<String>,
    /// True when the Docker daemon answered.
    pub daemon_running: bool,
    pub server_version: Option<String>,
    pub compose: Option<String>,
}

pub async fn docker(registry: &ShellRegistry, cwd: &Path) -> DockerRuntime {
    let version = probe(registry, cwd, &["docker", "--version"]).await;
    if version.is_none() {
        return DockerRuntime {
            available: false,
            version: None,
            daemon_running: false,
            server_version: None,
            compose: None,
        };
    }
    let (server, compose) = tokio::join!(
        probe(
            registry,
            cwd,
            &["docker", "info", "--format", "{{.ServerVersion}}"]
        ),
        probe(registry, cwd, &["docker", "compose", "version", "--short"]),
    );
    DockerRuntime {
        available: true,
        version,
        daemon_running: server.is_some(),
        server_version: server,
        compose,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(items: &[&str]) -> Vec<String> {
        words(items)
    }

    #[test]
    fn install_commands_per_manager() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        let pkgs = strings(&["zod", "@types/node"]);
        assert_eq!(
            install_command("pnpm", &[], false, p).unwrap(),
            strings(&["pnpm", "install"])
        );
        assert_eq!(
            install_command("pnpm", &pkgs, true, p).unwrap(),
            strings(&["pnpm", "add", "-D", "zod", "@types/node"])
        );
        assert_eq!(
            install_command("npm", &pkgs[..1], true, p).unwrap(),
            strings(&["npm", "install", "--save-dev", "zod"])
        );
        assert_eq!(
            install_command("uv", &[], false, p).unwrap(),
            strings(&["uv", "sync"])
        );
        assert_eq!(
            install_command("cargo", &strings(&["serde"]), false, p).unwrap(),
            strings(&["cargo", "add", "serde"])
        );
        let pip = install_command("pip", &[], false, p).unwrap();
        assert_eq!(pip.last().unwrap(), ".");
        std::fs::write(p.join("requirements.txt"), "").unwrap();
        let pip = install_command("pip", &[], false, p).unwrap();
        assert_eq!(pip.last().unwrap(), "requirements.txt");
    }

    #[test]
    fn run_commands_forward_arguments() {
        assert_eq!(
            run_command("npm", "test", &strings(&["--watch"])).unwrap(),
            strings(&["npm", "run", "test", "--", "--watch"])
        );
        assert_eq!(
            run_command("pnpm", "dev", &[]).unwrap(),
            strings(&["pnpm", "run", "dev"])
        );
        assert_eq!(
            run_command("cargo", "test", &strings(&["-p", "x"])).unwrap(),
            strings(&["cargo", "test", "-p", "x"])
        );
        assert!(run_command("pip", "x", &[]).is_err());
        assert!(run_command("npm", " ", &[]).is_err());
    }

    #[test]
    fn manager_choice_prefers_explicit_then_detected() {
        let detected = strings(&["pnpm", "npm"]);
        assert_eq!(
            choose_manager(None, &detected, SUPPORTED_MANAGERS).unwrap(),
            "pnpm"
        );
        assert_eq!(
            choose_manager(Some("YARN"), &detected, SUPPORTED_MANAGERS).unwrap(),
            "yarn"
        );
        assert!(choose_manager(Some("gradle"), &detected, SUPPORTED_MANAGERS).is_err());
        assert_eq!(
            choose_manager(None, &[], SUPPORTED_MANAGERS)
                .unwrap_err()
                .kind,
            ToolErrorKind::NotFound
        );
    }

    #[test]
    fn quoting_neutralizes_shell_syntax() {
        let posix = |a: &str| quote(ShellKind::Bash, a).unwrap();
        assert_eq!(posix("@types/node"), "@types/node");
        assert_eq!(posix("django>=4.2"), "'django>=4.2'");
        assert_eq!(posix("a; rm -rf /"), "'a; rm -rf /'");
        assert_eq!(posix("it's"), "'it'\\''s'");

        let ps = |a: &str| quote(ShellKind::Pwsh, a).unwrap();
        assert_eq!(ps("@types/node"), "'@types/node'");
        assert_eq!(ps("zod"), "zod");
        assert_eq!(ps("it's"), "'it''s'");
        assert_eq!(ps("{{.ServerVersion}}"), "'{{.ServerVersion}}'");

        let cmd = |a: &str| quote(ShellKind::Cmd, a);
        assert_eq!(cmd("react@^18").unwrap(), "\"react@^18\"");
        assert_eq!(cmd("@types/node").unwrap(), "@types/node");
        assert!(cmd("a\"b").is_err());
        assert!(cmd("%PATH%").is_err());
    }
}
