//! What a call asks for, in the user's words (ADR-0016): the line of an
//! authorization request and its detail.

use crate::text::{clip, line};
use orchestrator_core::ToolCall;
use serde_json::{Map, Value};

/// Characters of a long argument (a file's content) kept in the detail.
const DETAIL_TEXT: usize = 600;
/// Characters of the whole detail.
const DETAIL_MAX: usize = 2_000;

fn arg<'a>(call: &'a ToolCall, key: &str) -> Option<&'a str> {
    call.args
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|v| !v.is_empty())
}

fn flag(call: &ToolCall, key: &str) -> bool {
    call.args.get(key).and_then(Value::as_bool).unwrap_or(false)
}

fn list(call: &ToolCall, key: &str) -> Vec<String> {
    call.args
        .get(key)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn size(bytes: usize) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0).replace('.', ",")
    } else {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0)).replace('.', ",")
    }
}

fn quoted(text: &str) -> String {
    format!("`{}`", line(text, 160))
}

/// One line: "Executar `npm test`", "Escrever src/app.ts (2,1 KB)".
/// ` em dono/nome` when the call names the repository (ADR-0017).
fn github_repo(call: &ToolCall) -> String {
    arg(call, "repo")
        .map(|r| format!(" em {r}"))
        .unwrap_or_default()
}

/// Names of the secrets a call uses (`{{secret:NAME}}`, ADR-0020).
fn secret_names(call: &ToolCall) -> Vec<String> {
    let text = call.args.to_string();
    let mut names: Vec<String> = text
        .split("{{secret:")
        .skip(1)
        .filter_map(|rest| {
            rest.split_once("}}")
                .map(|(name, _)| name.trim().to_owned())
        })
        .filter(|name| !name.is_empty())
        .collect();
    names.sort();
    names.dedup();
    names
}

fn using_secrets(call: &ToolCall) -> String {
    match secret_names(call).as_slice() {
        [] => String::new(),
        [one] => format!(" (usa o segredo {one})"),
        many => format!(" (usa os segredos {})", many.join(", ")),
    }
}

fn number(call: &ToolCall) -> String {
    call.args
        .get("number")
        .and_then(Value::as_u64)
        .map_or_else(|| "?".to_owned(), |n| n.to_string())
}

pub fn summary(call: &ToolCall) -> String {
    let path = arg(call, "path").unwrap_or(".");
    match call.tool.as_str() {
        "filesystem.list" => format!("Listar {path}"),
        "filesystem.read" => format!("Ler {path}"),
        "filesystem.write" => {
            let bytes = arg(call, "content").map_or(0, str::len);
            if flag(call, "append") {
                format!("Acrescentar a {path} ({})", size(bytes))
            } else {
                format!("Escrever {path} ({})", size(bytes))
            }
        }
        "filesystem.move" => format!(
            "Mover {} para {}",
            arg(call, "from").unwrap_or("?"),
            arg(call, "to").unwrap_or("?")
        ),
        "filesystem.delete" if flag(call, "recursive") => {
            format!("Excluir {path} com tudo o que tem dentro")
        }
        "filesystem.delete" => format!("Excluir {path}"),
        "shell.execute" => format!(
            "Executar {}{}",
            quoted(arg(call, "command").unwrap_or("")),
            using_secrets(call)
        ),
        "web.fetch" => format!(
            "Ler a página {}{}",
            line(arg(call, "url").unwrap_or("?"), 160),
            using_secrets(call)
        ),
        "http.request" => format!(
            "Chamar a API: {} {}{}",
            arg(call, "method").unwrap_or("GET").to_uppercase(),
            line(arg(call, "url").unwrap_or("?"), 160),
            using_secrets(call)
        ),
        "secrets.list" => "Ver os nomes dos segredos".to_owned(),
        "process.start" => format!(
            "Iniciar o processo {}",
            quoted(arg(call, "command").unwrap_or(""))
        ),
        "process.stop" => format!("Encerrar o processo {}", arg(call, "id").unwrap_or("?")),
        "terminal.create" => "Abrir um terminal".to_owned(),
        "terminal.write" => format!(
            "Digitar no terminal: {}",
            quoted(arg(call, "data").unwrap_or(""))
        ),
        "terminal.close" => format!("Fechar o terminal {}", arg(call, "id").unwrap_or("?")),
        "git.add" if flag(call, "all") => "Preparar todas as alterações (git add)".to_owned(),
        "git.add" => format!("Preparar {} (git add)", list(call, "files").join(", ")),
        "git.commit" => format!(
            "Fazer commit: \"{}\"",
            line(arg(call, "message").unwrap_or(""), 120)
        ),
        "git.push" => {
            let target = [arg(call, "remote"), arg(call, "branch")]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join(" ");
            let force = if flag(call, "force") {
                " com --force-with-lease"
            } else {
                ""
            };
            if target.is_empty() {
                format!("Enviar para o remoto (git push){force}")
            } else {
                format!("Enviar para o remoto: {target}{force}")
            }
        }
        "git.pull" => "Trazer do remoto (git pull)".to_owned(),
        "git.fetch" => match arg(call, "remote") {
            Some(remote) => format!("Buscar do remoto {remote} (git fetch)"),
            None => "Buscar do remoto (git fetch)".to_owned(),
        },
        "github.pr.create" => {
            format!(
                "Abrir o pull request \"{}\"{}{}",
                line(arg(call, "title").unwrap_or(""), 100),
                match (arg(call, "head"), arg(call, "base")) {
                    (Some(head), Some(base)) => format!(" ({head} → {base})"),
                    (None, Some(base)) => format!(" (→ {base})"),
                    (Some(head), None) => format!(" ({head} →)"),
                    (None, None) => String::new(),
                },
                github_repo(call)
            ) + if flag(call, "draft") {
                " como rascunho"
            } else {
                ""
            }
        }
        "github.pr.merge" => format!(
            "Fazer merge ({}) do PR #{}{}{}",
            arg(call, "method").unwrap_or("merge"),
            number(call),
            github_repo(call),
            if flag(call, "deleteBranch") {
                " e apagar a branch"
            } else {
                ""
            }
        ),
        "github.pr.comment" => format!(
            "Comentar no PR #{}{}: \"{}\"",
            number(call),
            github_repo(call),
            line(arg(call, "body").unwrap_or(""), 80)
        ),
        "github.issue.create" => format!(
            "Abrir a issue \"{}\"{}",
            line(arg(call, "title").unwrap_or(""), 100),
            github_repo(call)
        ),
        "github.issue.comment" => format!(
            "Comentar na issue #{}{}: \"{}\"",
            number(call),
            github_repo(call),
            line(arg(call, "body").unwrap_or(""), 80)
        ),
        "git.checkout" => format!("Trocar para {}", arg(call, "target").unwrap_or("?")),
        "git.branch" => match (arg(call, "create"), arg(call, "delete")) {
            (Some(name), _) => format!("Criar a branch {name}"),
            (_, Some(name)) => format!("Apagar a branch {name}"),
            _ => "Listar as branches".to_owned(),
        },
        "git.reset" => format!(
            "git reset {}{}",
            arg(call, "mode").unwrap_or("mixed"),
            arg(call, "target")
                .map(|t| format!(" {t}"))
                .unwrap_or_default()
        ),
        "git.stash" => format!("git stash {}", arg(call, "action").unwrap_or("push")),
        "package.install" => {
            let packages = list(call, "packages");
            if packages.is_empty() {
                "Instalar as dependências do projeto".to_owned()
            } else {
                format!("Instalar {}", packages.join(", "))
            }
        }
        "package.run" => format!("Rodar o script {}", arg(call, "script").unwrap_or("?")),
        "project.open" => format!("Abrir o projeto {path}"),
        "project.discover" => "Procurar projetos no disco".to_owned(),
        "memory.save" => format!(
            "Gravar na memória do projeto: {}",
            line(arg(call, "title").unwrap_or(""), 120)
        ),
        "decision.save" => format!(
            "Registrar a decisão: {}",
            line(arg(call, "title").unwrap_or(""), 120)
        ),
        "agent.delegate" => format!(
            "Criar um subagente: {}",
            line(arg(call, "title").unwrap_or(""), 120)
        ),
        "agent.finish" => "Entregar o resultado da task".to_owned(),
        other => other.to_owned(),
    }
}

fn shorten(value: &Value) -> Value {
    match value {
        Value::String(text) if text.chars().count() > DETAIL_TEXT => Value::String(format!(
            "{} (… {} caracteres)",
            clip(text, DETAIL_TEXT),
            text.chars().count()
        )),
        Value::Array(items) => Value::Array(items.iter().map(shorten).collect()),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(k, v)| (k.clone(), shorten(v)))
                .collect::<Map<_, _>>(),
        ),
        other => other.clone(),
    }
}

/// The arguments, with long texts cut: what the user reads before
/// deciding.
pub fn detail(call: &ToolCall) -> Option<String> {
    if call.args.is_null() || call.args.as_object().is_some_and(Map::is_empty) {
        return None;
    }
    let text = serde_json::to_string_pretty(&shorten(&call.args)).ok()?;
    Some(clip(&text, DETAIL_MAX))
}

#[cfg(test)]
mod tests {
    use super::*;
    use orchestrator_core::CallOrigin;
    use serde_json::json;

    fn call(tool: &str, args: Value) -> ToolCall {
        ToolCall::new(tool, args, CallOrigin::User)
    }

    #[test]
    fn summaries_read_as_the_user_would_say_them() {
        assert_eq!(
            summary(&call("shell.execute", json!({"command": "npm  test"}))),
            "Executar `npm test`"
        );
        assert_eq!(
            summary(&call(
                "filesystem.write",
                json!({"path": "src/a.ts", "content": "x".repeat(2150)})
            )),
            "Escrever src/a.ts (2,1 KB)"
        );
        assert_eq!(
            summary(&call(
                "git.push",
                json!({"remote": "origin", "branch": "main", "force": true})
            )),
            "Enviar para o remoto: origin main com --force-with-lease"
        );
        assert_eq!(
            summary(&call(
                "filesystem.delete",
                json!({"path": "build", "recursive": true})
            )),
            "Excluir build com tudo o que tem dentro"
        );
        assert_eq!(
            summary(&call("agent.delegate", json!({"title": "Testes"}))),
            "Criar um subagente: Testes"
        );
        assert_eq!(summary(&call("x.y", json!({}))), "x.y");
        // Internet and secrets (ADR-0020): the names, never the values.
        assert_eq!(
            summary(&call(
                "http.request",
                json!({"method": "post", "url": "https://api.x/v1/itens",
                       "headers": {"Authorization": "Bearer {{secret:API_X}}"}})
            )),
            "Chamar a API: POST https://api.x/v1/itens (usa o segredo API_X)"
        );
        assert_eq!(
            summary(&call("web.fetch", json!({"url": "https://example.com"}))),
            "Ler a página https://example.com"
        );
        // GitHub (ADR-0017).
        assert_eq!(
            summary(&call(
                "github.pr.create",
                json!({"title": "Retentativas", "base": "main", "draft": true})
            )),
            "Abrir o pull request \"Retentativas\" (→ main) como rascunho"
        );
        assert_eq!(
            summary(&call(
                "github.pr.merge",
                json!({"number": 12, "method": "squash", "repo": "time/app", "deleteBranch": true})
            )),
            "Fazer merge (squash) do PR #12 em time/app e apagar a branch"
        );
        assert_eq!(
            summary(&call(
                "github.issue.comment",
                json!({"number": 3, "body": "Resolvido\nem main"})
            )),
            "Comentar na issue #3: \"Resolvido em main\""
        );
        assert_eq!(
            summary(&call("git.fetch", json!({}))),
            "Buscar do remoto (git fetch)"
        );
    }

    #[test]
    fn the_detail_cuts_long_content_but_keeps_the_rest() {
        let long = "a".repeat(5_000);
        let text = detail(&call(
            "filesystem.write",
            json!({"path": "big.txt", "content": long}),
        ))
        .unwrap();
        assert!(text.contains("big.txt"));
        assert!(text.contains("5000 caracteres"));
        assert!(text.chars().count() <= DETAIL_MAX);
        assert_eq!(detail(&call("git.status", json!({}))), None);
    }
}
