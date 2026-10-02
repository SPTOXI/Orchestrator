//! Internet access for the AIs (ADR-0020): `web.fetch` (read a page) and
//! `http.request` (call any API), and the user's secrets, used by name.
//!
//! A secret is written `{{secret:NAME}}` in a URL, header or body (or in
//! the `env` of `shell.execute`) and replaced by its value only when the
//! request goes out. Its value never reaches the model or the history:
//! the arguments keep the placeholder, and every value is masked in what
//! comes back.

use orchestrator_core::{ToolError, ToolErrorKind};
use orchestrator_git::github::Secret;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::OnceLock;
use std::time::Duration;

/// Body read by default (1 MiB) and at most (20 MiB).
pub const DEFAULT_MAX_BYTES: u64 = 1024 * 1024;
const MAX_MAX_BYTES: u64 = 20 * 1024 * 1024;
pub const DEFAULT_TIMEOUT_MS: u64 = 60_000;
const MAX_REDIRECTS: usize = 10;
const SECRET_OPEN: &str = "{{secret:";
const SECRET_CLOSE: &str = "}}";
/// Values shorter than this are not masked (they would garble the text).
const MIN_MASKED: usize = 4;
const MASK: &str = "***";

/// The user's secrets by name.
pub type Secrets = BTreeMap<String, Secret>;

/// Whether `text` names a secret.
pub fn mentions_secret(text: &str) -> bool {
    text.contains(SECRET_OPEN)
}

/// `text` with every `{{secret:NAME}}` replaced by the value.
pub fn expand(text: &str, secrets: &Secrets) -> Result<String, ToolError> {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find(SECRET_OPEN) {
        out.push_str(&rest[..start]);
        let after = &rest[start + SECRET_OPEN.len()..];
        let end = after.find(SECRET_CLOSE).ok_or_else(|| {
            ToolError::invalid_args("unclosed {{secret:NAME}} placeholder (missing \"}}\")")
        })?;
        let name = after[..end].trim();
        let secret = secrets
            .get(name)
            .ok_or_else(|| missing_secret(name, secrets))?;
        out.push_str(secret.expose());
        rest = &after[end + SECRET_CLOSE.len()..];
    }
    out.push_str(rest);
    Ok(out)
}

fn missing_secret(name: &str, secrets: &Secrets) -> ToolError {
    let known = if secrets.is_empty() {
        "no secret is saved yet".to_owned()
    } else {
        format!(
            "saved: {}",
            secrets.keys().cloned().collect::<Vec<_>>().join(", ")
        )
    };
    ToolError::invalid_args(format!(
        "the secret \"{name}\" does not exist ({known}). The user saves secrets in the Orchestrator: \
         Configurações tab, \"Políticas e segredos\" (or the Autonomia chip)."
    ))
}

/// `text` with the value of every secret hidden.
pub fn mask(text: &str, secrets: &Secrets) -> String {
    let mut out = text.to_owned();
    for secret in secrets.values() {
        let value = secret.expose();
        if value.len() >= MIN_MASKED && out.contains(value) {
            out = out.replace(value, MASK);
        }
    }
    out
}

/// [`mask`] on every string (and object key) of `value`.
pub fn mask_value(value: &mut Value, secrets: &Secrets) {
    match value {
        Value::String(text) => *text = mask(text, secrets),
        Value::Array(items) => items.iter_mut().for_each(|v| mask_value(v, secrets)),
        Value::Object(map) => {
            let entries = std::mem::take(map);
            for (key, mut item) in entries {
                mask_value(&mut item, secrets);
                map.insert(mask(&key, secrets), item);
            }
        }
        _ => {}
    }
}

// ------------------------------------------------------------- arguments --

/// What `web.fetch` returns.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize, schemars::JsonSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum FetchFormat {
    /// HTML turned into readable text (other types as they are).
    #[default]
    Text,
    /// The body as it came.
    Raw,
}

/// Arguments of `web.fetch`.
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FetchArgs {
    /// http(s) URL. May contain {{secret:NAME}}.
    pub url: String,
    /// Extra request headers; values may contain {{secret:NAME}}.
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    /// "text" (default: an HTML page becomes readable text) or "raw".
    #[serde(default)]
    pub format: FetchFormat,
    /// Body bytes read (default 1 MiB, at most 20 MiB).
    #[serde(default)]
    pub max_bytes: Option<u64>,
    /// Default 60000.
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "UPPERCASE")]
pub enum HttpMethod {
    #[default]
    Get,
    Post,
    Put,
    Patch,
    Delete,
    Head,
}

impl HttpMethod {
    fn reqwest(self) -> reqwest::Method {
        match self {
            Self::Get => reqwest::Method::GET,
            Self::Post => reqwest::Method::POST,
            Self::Put => reqwest::Method::PUT,
            Self::Patch => reqwest::Method::PATCH,
            Self::Delete => reqwest::Method::DELETE,
            Self::Head => reqwest::Method::HEAD,
        }
    }
}

/// Arguments of `http.request`.
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RequestArgs {
    /// Default GET.
    #[serde(default)]
    pub method: HttpMethod,
    /// http(s) URL. May contain {{secret:NAME}}.
    pub url: String,
    /// Request headers; values may contain {{secret:NAME}}.
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    /// Body as text. May contain {{secret:NAME}}.
    #[serde(default)]
    pub body: Option<String>,
    /// Body as JSON (sent with content-type application/json). Strings may
    /// contain {{secret:NAME}}.
    #[serde(default)]
    pub json: Option<Value>,
    /// Body bytes read (default 1 MiB, at most 20 MiB).
    #[serde(default)]
    pub max_bytes: Option<u64>,
    /// Default 60000.
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

// --------------------------------------------------------------- outputs --

/// Output of `web.fetch`. A 4xx/5xx is not a tool error: `ok` says.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FetchOutput {
    /// After redirects.
    pub url: String,
    pub status: u16,
    pub ok: bool,
    pub content_type: Option<String>,
    /// `<title>` of an HTML page.
    pub title: Option<String>,
    pub content: String,
    /// Not text (image, PDF, archive…): `content` is empty.
    pub binary: bool,
    pub truncated: bool,
    /// Body bytes read.
    pub bytes: usize,
}

/// Output of `http.request`. A 4xx/5xx is not a tool error: `ok` says.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestOutput {
    pub url: String,
    pub status: u16,
    pub ok: bool,
    /// Lowercase names; repeated headers joined with ", ".
    pub headers: BTreeMap<String, String>,
    /// Parsed when the response is JSON; text otherwise; null for no body
    /// or a binary one.
    pub body: Value,
    pub binary: bool,
    pub truncated: bool,
    pub bytes: usize,
}

/// Names of the secrets (`secrets.list`); never the values.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SecretList {
    pub names: Vec<String>,
    /// How to use one.
    pub usage: String,
}

pub fn list(secrets: &Secrets) -> SecretList {
    SecretList {
        names: secrets.keys().cloned().collect(),
        usage: "Write {{secret:NAME}} in a URL, header or body of web.fetch/http.request, or in an env \
                value of shell.execute. The value is never shown to you."
            .into(),
    }
}

// -------------------------------------------------------------- requests --

fn client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::limited(MAX_REDIRECTS))
            .connect_timeout(Duration::from_secs(20))
            .user_agent(concat!("Orchestrator/", env!("CARGO_PKG_VERSION")))
            .build()
            .expect("HTTP client")
    })
}

fn parse_url(url: &str) -> Result<reqwest::Url, ToolError> {
    let parsed = reqwest::Url::parse(url.trim())
        .map_err(|e| ToolError::invalid_args(format!("invalid URL: {e}")))?;
    match parsed.scheme() {
        "http" | "https" => Ok(parsed),
        other => Err(ToolError::invalid_args(format!(
            "only http and https URLs are supported (got {other}:)"
        ))),
    }
}

fn header_map(
    headers: &BTreeMap<String, String>,
    secrets: &Secrets,
) -> Result<reqwest::header::HeaderMap, ToolError> {
    let mut map = reqwest::header::HeaderMap::new();
    for (name, value) in headers {
        let name = reqwest::header::HeaderName::from_bytes(name.trim().as_bytes())
            .map_err(|_| ToolError::invalid_args(format!("invalid header name: {name}")))?;
        let value = reqwest::header::HeaderValue::from_str(&expand(value, secrets)?)
            .map_err(|_| ToolError::invalid_args(format!("invalid value for header {name}")))?;
        map.append(name, value);
    }
    Ok(map)
}

/// Strings of a JSON body with their secrets expanded.
fn expand_json(value: &Value, secrets: &Secrets) -> Result<Value, ToolError> {
    Ok(match value {
        Value::String(text) => Value::String(expand(text, secrets)?),
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|v| expand_json(v, secrets))
                .collect::<Result<_, _>>()?,
        ),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(k, v)| Ok((k.clone(), expand_json(v, secrets)?)))
                .collect::<Result<_, ToolError>>()?,
        ),
        other => other.clone(),
    })
}

fn transport_error(err: reqwest::Error, timeout_ms: u64) -> ToolError {
    // Never the URL: it may carry a secret.
    let err = err.without_url();
    if err.is_timeout() {
        ToolError::new(
            ToolErrorKind::Io,
            format!("timed out after {timeout_ms} ms"),
        )
    } else if err.is_connect() {
        ToolError::new(ToolErrorKind::Io, format!("cannot connect: {err}"))
    } else {
        ToolError::new(ToolErrorKind::Io, format!("request failed: {err}"))
    }
}

struct Body {
    bytes: Vec<u8>,
    truncated: bool,
}

async fn read_body(
    mut response: reqwest::Response,
    max: usize,
    timeout_ms: u64,
) -> Result<Body, ToolError> {
    let mut bytes = Vec::new();
    let mut truncated = false;
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| transport_error(e, timeout_ms))?
    {
        let room = max.saturating_sub(bytes.len());
        if chunk.len() > room {
            bytes.extend_from_slice(&chunk[..room]);
            truncated = true;
            break;
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(Body { bytes, truncated })
}

fn limits(max_bytes: Option<u64>, timeout_ms: Option<u64>) -> (usize, u64) {
    let max = max_bytes
        .unwrap_or(DEFAULT_MAX_BYTES)
        .clamp(1, MAX_MAX_BYTES) as usize;
    (max, timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS).max(1))
}

fn content_type(response: &reqwest::Response) -> Option<String> {
    response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned)
}

fn is_text(content_type: Option<&str>) -> bool {
    let Some(kind) = content_type else {
        return true;
    };
    let kind = kind.to_ascii_lowercase();
    kind.starts_with("text/")
        || [
            "json",
            "xml",
            "javascript",
            "yaml",
            "x-www-form-urlencoded",
            "graphql",
            "csv",
        ]
        .iter()
        .any(|t| kind.contains(t))
}

fn is_html(content_type: Option<&str>) -> bool {
    content_type.is_some_and(|t| t.to_ascii_lowercase().contains("html"))
}

/// Bytes as text, in the charset the server (or the page) says.
fn decode(bytes: &[u8], content_type: Option<&str>) -> String {
    let declared = content_type
        .and_then(|t| {
            t.to_ascii_lowercase()
                .split("charset=")
                .nth(1)
                .map(str::to_owned)
        })
        .or_else(|| sniff_meta_charset(bytes));
    let encoding = declared
        .as_deref()
        .map(|label| {
            label.trim_matches(|c: char| c == '"' || c == '\'' || c.is_whitespace() || c == ';')
        })
        .and_then(|label| encoding_rs::Encoding::for_label(label.as_bytes()))
        .unwrap_or(encoding_rs::UTF_8);
    encoding.decode(bytes).0.into_owned()
}

/// `<meta charset=…>` in the first bytes of an HTML page.
fn sniff_meta_charset(bytes: &[u8]) -> Option<String> {
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(4096)]).to_ascii_lowercase();
    let at = head.find("charset=")? + "charset=".len();
    let value: String = head[at..]
        .trim_start_matches(['"', '\''])
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect();
    (!value.is_empty()).then_some(value)
}

pub async fn fetch(args: FetchArgs, secrets: &Secrets) -> Result<FetchOutput, ToolError> {
    let url = parse_url(&expand(&args.url, secrets)?)?;
    let headers = header_map(&args.headers, secrets)?;
    let (max, timeout_ms) = limits(args.max_bytes, args.timeout_ms);
    let response = client()
        .get(url)
        .headers(headers)
        .timeout(Duration::from_millis(timeout_ms))
        .send()
        .await
        .map_err(|e| transport_error(e, timeout_ms))?;
    let status = response.status();
    let final_url = response.url().to_string();
    let content_type = content_type(&response);
    let body = read_body(response, max, timeout_ms).await?;
    let binary = !is_text(content_type.as_deref());
    let (title, content) = if binary {
        (None, String::new())
    } else {
        let text = decode(&body.bytes, content_type.as_deref());
        if is_html(content_type.as_deref()) && args.format == FetchFormat::Text {
            (html_title(&text), html_to_text(&text))
        } else {
            (
                is_html(content_type.as_deref())
                    .then(|| html_title(&text))
                    .flatten(),
                text,
            )
        }
    };
    Ok(FetchOutput {
        url: final_url,
        status: status.as_u16(),
        ok: status.is_success(),
        content_type,
        title,
        content,
        binary,
        truncated: body.truncated,
        bytes: body.bytes.len(),
    })
}

pub async fn request(args: RequestArgs, secrets: &Secrets) -> Result<RequestOutput, ToolError> {
    if args.body.is_some() && args.json.is_some() {
        return Err(ToolError::invalid_args("use either body or json, not both"));
    }
    let url = parse_url(&expand(&args.url, secrets)?)?;
    let headers = header_map(&args.headers, secrets)?;
    let (max, timeout_ms) = limits(args.max_bytes, args.timeout_ms);
    let mut builder = client()
        .request(args.method.reqwest(), url)
        .headers(headers)
        .timeout(Duration::from_millis(timeout_ms));
    if let Some(body) = &args.body {
        builder = builder.body(expand(body, secrets)?);
    }
    if let Some(json) = &args.json {
        builder = builder.json(&expand_json(json, secrets)?);
    }
    let response = builder
        .send()
        .await
        .map_err(|e| transport_error(e, timeout_ms))?;
    let status = response.status();
    let final_url = response.url().to_string();
    let content_type = content_type(&response);
    let mut headers: BTreeMap<String, String> = BTreeMap::new();
    for (name, value) in response.headers() {
        let value = String::from_utf8_lossy(value.as_bytes()).into_owned();
        headers
            .entry(name.as_str().to_owned())
            .and_modify(|v| {
                v.push_str(", ");
                v.push_str(&value);
            })
            .or_insert(value);
    }
    let body = read_body(response, max, timeout_ms).await?;
    let binary = !body.bytes.is_empty() && !is_text(content_type.as_deref());
    let value = if body.bytes.is_empty() || binary {
        Value::Null
    } else {
        let text = decode(&body.bytes, content_type.as_deref());
        let json = content_type
            .as_deref()
            .is_some_and(|t| t.to_ascii_lowercase().contains("json"));
        match (json && !body.truncated)
            .then(|| serde_json::from_str::<Value>(&text).ok())
            .flatten()
        {
            Some(parsed) => parsed,
            None => Value::String(text),
        }
    };
    Ok(RequestOutput {
        url: final_url,
        status: status.as_u16(),
        ok: status.is_success(),
        headers,
        body: value,
        binary,
        truncated: body.truncated,
        bytes: body.bytes.len(),
    })
}

// ------------------------------------------------------------------ HTML --

/// The `<title>` of a page.
pub fn html_title(html: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let open = lower.find("<title")?;
    let start = open + lower[open..].find('>')? + 1;
    let end = start + lower[start..].find("</title")?;
    let title = collapse_spaces(&decode_entities(&html[start..end]));
    (!title.is_empty()).then_some(title)
}

/// A page as readable text: no scripts, styles, comments or tags; block
/// elements become line breaks; entities decoded.
pub fn html_to_text(html: &str) -> String {
    let mut text = String::with_capacity(html.len() / 2);
    let lower = html.to_ascii_lowercase();
    let mut i = 0;
    while i < html.len() {
        let rest = &lower[i..];
        if rest.starts_with("<!--") {
            i += rest.find("-->").map_or(rest.len(), |end| end + 3);
            continue;
        }
        if let Some(inner) = rest.strip_prefix('<') {
            let Some(close) = rest.find('>') else {
                break;
            };
            let closing = inner.starts_with('/');
            let tag = rest[1..close].trim_start_matches('/').trim();
            let name: String = tag
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric())
                .collect();
            if ["script", "style", "noscript", "template", "svg", "head"].contains(&name.as_str())
                && !closing
            {
                // Skip to the matching end tag.
                let end_tag = format!("</{name}");
                i += rest.find(&end_tag).map_or(rest.len(), |at| {
                    at + rest[at..].find('>').map_or(end_tag.len(), |g| g + 1)
                });
                continue;
            }
            if [
                "br",
                "p",
                "div",
                "li",
                "tr",
                "h1",
                "h2",
                "h3",
                "h4",
                "h5",
                "h6",
                "section",
                "article",
                "header",
                "footer",
                "ul",
                "ol",
                "table",
                "pre",
                "blockquote",
                "hr",
                "title",
                "main",
                "nav",
            ]
            .contains(&name.as_str())
            {
                // One line break per block boundary; <br> always breaks.
                if name == "br" || !text.ends_with('\n') {
                    text.push('\n');
                }
            } else if name == "td" || name == "th" {
                text.push('\t');
            }
            i += close + 1;
            continue;
        }
        let next = rest.find('<').unwrap_or(rest.len());
        text.push_str(&html[i..i + next]);
        i += next;
    }
    let decoded = decode_entities(&text);
    let mut out = String::with_capacity(decoded.len());
    let mut blank = 0;
    for line in decoded.lines() {
        let line = collapse_spaces(line);
        if line.is_empty() {
            blank += 1;
            if blank > 1 || out.is_empty() {
                continue;
            }
        } else {
            blank = 0;
        }
        out.push_str(&line);
        out.push('\n');
    }
    out.trim_end().to_owned()
}

fn collapse_spaces(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn decode_entities(text: &str) -> String {
    if !text.contains('&') {
        return text.to_owned();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        let tail = &rest[at..];
        let end = tail[..tail.len().min(12)].find(';');
        let decoded = end.and_then(|end| {
            let entity = &tail[1..end];
            let ch = match entity {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" | "#39" => Some('\''),
                "nbsp" => Some(' '),
                _ if entity.starts_with("#x") || entity.starts_with("#X") => {
                    u32::from_str_radix(&entity[2..], 16)
                        .ok()
                        .and_then(char::from_u32)
                }
                _ if entity.starts_with('#') => entity[1..].parse().ok().and_then(char::from_u32),
                _ => None,
            };
            ch.map(|c| (c, end + 1))
        });
        match decoded {
            Some((ch, len)) => {
                out.push(ch);
                rest = &tail[len..];
            }
            None => {
                out.push('&');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secrets() -> Secrets {
        let mut s = Secrets::new();
        s.insert("API_KEY".into(), Secret::new("sk-segredo-123").unwrap());
        s
    }

    #[test]
    fn secrets_are_expanded_and_masked() {
        let s = secrets();
        assert_eq!(
            expand("Bearer {{secret:API_KEY}}", &s).unwrap(),
            "Bearer sk-segredo-123"
        );
        assert_eq!(
            expand("{{ secret:API_KEY }}x", &s).unwrap(),
            "{{ secret:API_KEY }}x"
        );
        let err = expand("{{secret:OUTRO}}", &s).unwrap_err();
        assert_eq!(err.kind, ToolErrorKind::InvalidArgs);
        assert!(
            err.message.contains("OUTRO") && err.message.contains("API_KEY"),
            "{}",
            err.message
        );
        assert!(expand("{{secret:API_KEY", &s).is_err());
        assert_eq!(mask("chave=sk-segredo-123;", &s), "chave=***;");
        let mut value = serde_json::json!({"echo": ["sk-segredo-123"], "sk-segredo-123": 1});
        mask_value(&mut value, &s);
        assert_eq!(value, serde_json::json!({"echo": ["***"], "***": 1}));
        assert!(mentions_secret("x {{secret:A}}") && !mentions_secret("x {secret:A}"));
    }

    #[test]
    fn html_becomes_readable_text() {
        let html = "<html><head><title>Preço &amp; Cia</title><style>p{}</style></head>\
                    <body><script>var x = '<p>';</script><h1>Olá</h1><p>Um&nbsp;parágrafo <b>forte</b>.</p>\
                    <!-- nota --><ul><li>a</li><li>b &#8212; c</li></ul></body></html>";
        assert_eq!(html_title(html).as_deref(), Some("Preço & Cia"));
        assert_eq!(html_to_text(html), "Olá\nUm parágrafo forte.\na\nb — c");
    }

    #[test]
    fn charsets_are_honoured() {
        let latin1 = b"<meta charset=\"iso-8859-1\"><p>a\xe7\xe3o</p>";
        assert!(decode(latin1, Some("text/html")).contains("ação"));
        assert_eq!(
            decode(b"a\xe7", Some("text/plain; charset=ISO-8859-1")),
            "aç"
        );
        assert_eq!(decode("ação".as_bytes(), None), "ação");
        assert!(is_text(Some("application/json; charset=utf-8")));
        assert!(!is_text(Some("image/png")));
    }

    #[test]
    fn only_web_urls() {
        assert!(parse_url("https://example.com/a").is_ok());
        assert!(parse_url("file:///etc/passwd").is_err());
        assert!(parse_url("nada").is_err());
    }
}
