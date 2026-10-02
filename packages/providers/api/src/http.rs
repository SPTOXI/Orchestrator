//! HTTP transport shared by every protocol: requests with cancellation,
//! error mapping and response framing (SSE, NDJSON, whole body).

use crate::config::StreamFormat;
use crate::jsonpath;
use futures_util::StreamExt;
use orchestrator_providers::{ProviderError, ProviderErrorKind};
use serde_json::Value;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

/// A stream that sends nothing for this long is considered dead.
const IDLE_TIMEOUT: Duration = Duration::from_secs(300);
/// Error bodies are read up to this size.
const MAX_ERROR_BODY: usize = 64 * 1024;
/// Repeats of a request the server did not take (ADR-0018).
pub const MAX_RETRIES: u32 = 2;
const RETRY_BASE: Duration = Duration::from_secs(2);
const MAX_RETRY_WAIT: Duration = Duration::from_secs(60);
/// Timeout, rate limit, server errors and overload (529, Anthropic).
const RETRY_STATUSES: &[u16] = &[408, 429, 500, 502, 503, 504, 529];

/// A request about to be repeated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Retry {
    /// 1 for the first repeat.
    pub attempt: u32,
    pub wait: Duration,
    /// What went wrong, as the API said it.
    pub reason: String,
    pub kind: RetryKind,
}

/// Why a request is repeated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RetryKind {
    /// The server did not take it: the same request after `wait`.
    Wait,
    /// The account's credit pays only `affordable` output tokens (a 402
    /// that says so, as OpenRouter's): sent again at once asking for at
    /// most `tokens`.
    SmallerOutput { affordable: u32, tokens: u32 },
    /// This connection could not serve it: the fallback connection
    /// (`connection`, by name) is asked with `model`.
    Fallback { connection: String, model: String },
}

struct Failure {
    error: ProviderError,
    retryable: bool,
    retry_after: Option<Duration>,
}

impl Failure {
    fn final_(error: ProviderError) -> Self {
        Self {
            error,
            retryable: false,
            retry_after: None,
        }
    }
}

/// `retry-after-ms` (milliseconds) or `retry-after` (seconds; an HTTP date
/// is ignored and the default backoff applies).
fn retry_after(headers: &reqwest::header::HeaderMap) -> Option<Duration> {
    let text = |name: &str| {
        headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::trim)
    };
    if let Some(ms) = text("retry-after-ms").and_then(|v| v.parse::<f64>().ok()) {
        return (ms.is_finite() && ms >= 0.0).then(|| Duration::from_millis(ms as u64));
    }
    text("retry-after")
        .and_then(|v| v.parse::<f64>().ok())
        .filter(|s| s.is_finite() && *s >= 0.0)
        .map(Duration::from_secs_f64)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
    Delete,
}

/// A fully built request. Headers may carry the API key: never log them.
#[derive(Debug, Clone)]
pub struct HttpCall {
    pub method: Method,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<Value>,
}

impl HttpCall {
    pub fn post(url: String, body: Value) -> Self {
        Self {
            method: Method::Post,
            url,
            headers: Vec::new(),
            body: Some(body),
        }
    }

    pub fn get(url: String) -> Self {
        Self {
            method: Method::Get,
            url,
            headers: Vec::new(),
            body: None,
        }
    }

    pub fn delete(url: String, body: Value) -> Self {
        Self {
            method: Method::Delete,
            url,
            headers: Vec::new(),
            body: Some(body),
        }
    }

    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }
}

/// One unit of a response: an SSE event, an NDJSON line or a whole body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    /// SSE `event:` name, if any.
    pub event: Option<String>,
    pub data: String,
}

#[derive(Clone)]
pub struct HttpClient {
    client: reqwest::Client,
}

impl HttpClient {
    pub fn new() -> Result<Self, ProviderError> {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(20))
            .user_agent(concat!("Orchestrator/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| ProviderError::internal(format!("cannot build HTTP client: {e}")))?;
        Ok(Self { client })
    }

    /// Sends `call`; non-2xx statuses become typed errors with the API's
    /// own message.
    pub async fn send(
        &self,
        call: &HttpCall,
        cancel: &CancellationToken,
    ) -> Result<reqwest::Response, ProviderError> {
        self.send_once(call, cancel, None)
            .await
            .map_err(|f| f.error)
    }

    /// [`Self::send`], repeating a request the server did not take (rate
    /// limit, overload, a 5xx, a connection that never opened) up to
    /// [`MAX_RETRIES`] times. The wait is what the server asks for
    /// (`retry-after`, capped) or 2 s, then 4 s. `on_retry` is told before
    /// each wait; cancelling ends the wait (ADR-0018). A server that does
    /// not answer within `first_response` is not waited for again: it is
    /// queueing requests it cannot serve.
    pub async fn send_retrying(
        &self,
        call: &HttpCall,
        cancel: &CancellationToken,
        on_retry: &(dyn Fn(&Retry) + Send + Sync),
        first_response: Option<Duration>,
    ) -> Result<reqwest::Response, ProviderError> {
        let mut attempt = 0;
        loop {
            let failure = match self.send_once(call, cancel, first_response).await {
                Ok(response) => return Ok(response),
                Err(failure) => failure,
            };
            if !failure.retryable || attempt >= MAX_RETRIES {
                return Err(failure.error);
            }
            attempt += 1;
            let wait = failure
                .retry_after
                .unwrap_or(RETRY_BASE * 2u32.pow(attempt - 1))
                .min(MAX_RETRY_WAIT);
            on_retry(&Retry {
                attempt,
                wait,
                reason: failure.error.message.clone(),
                kind: RetryKind::Wait,
            });
            tokio::select! {
                () = tokio::time::sleep(wait) => {}
                () = cancel.cancelled() => return Err(ProviderError::cancelled("request cancelled")),
            }
        }
    }

    async fn send_once(
        &self,
        call: &HttpCall,
        cancel: &CancellationToken,
        first_response: Option<Duration>,
    ) -> Result<reqwest::Response, Failure> {
        let mut request = match call.method {
            Method::Get => self.client.get(&call.url),
            Method::Post => self.client.post(&call.url),
            Method::Delete => self.client.delete(&call.url),
        };
        for (name, value) in &call.headers {
            request = request.header(name, value);
        }
        if let Some(body) = &call.body {
            request = request.json(body);
        }
        let send = async {
            match first_response {
                Some(limit) => tokio::time::timeout(limit, request.send())
                    .await
                    .map_err(|_| limit),
                None => Ok(request.send().await),
            }
        };
        let response = tokio::select! {
            result = send => match result {
                Err(limit) => return Err(Failure::final_(not_started(limit))),
                Ok(Ok(response)) => response,
                Ok(Err(err)) => {
                    // Nothing reached the server: safe to send again.
                    let retryable = err.is_connect();
                    return Err(Failure {
                        error: transport_error(err, &call.url),
                        retryable,
                        retry_after: None,
                    });
                }
            },
            () = cancel.cancelled() => {
                return Err(Failure::final_(ProviderError::cancelled("request cancelled")))
            }
        };
        let status = response.status();
        if status.is_success() {
            return Ok(response);
        }
        let retry_after = retry_after(response.headers());
        let body = read_limited(response, cancel).await.unwrap_or_default();
        Err(Failure {
            error: status_error(status.as_u16(), &body),
            retryable: RETRY_STATUSES.contains(&status.as_u16()),
            retry_after,
        })
    }

    /// Sends and parses a JSON response.
    pub async fn json(
        &self,
        call: &HttpCall,
        cancel: &CancellationToken,
    ) -> Result<Value, ProviderError> {
        let response = self.send(call, cancel).await?;
        let text = read_all(response, cancel).await?;
        serde_json::from_str(&text).map_err(|e| {
            ProviderError::failed(format!(
                "response is not JSON ({e}): {}",
                truncate(&text, 300)
            ))
        })
    }
}

/// Reads frames from a response in the given format.
pub struct FrameReader {
    response: Option<reqwest::Response>,
    format: StreamFormat,
    lines: LineSplitter,
    sse: SseState,
    pending: std::collections::VecDeque<Frame>,
    done: bool,
    /// Until the first frame: when the server must have started answering.
    first_frame: Option<(tokio::time::Instant, Duration)>,
}

impl FrameReader {
    pub fn new(response: reqwest::Response, format: StreamFormat) -> Self {
        Self {
            response: Some(response),
            format,
            lines: LineSplitter::default(),
            sse: SseState::default(),
            pending: Default::default(),
            done: false,
            first_frame: None,
        }
    }

    /// Fails with [`not_started`] when no frame arrives within `limit`
    /// (keep-alive comments do not count: a server that only sends them
    /// has not started on the request).
    pub fn with_first_frame_limit(mut self, limit: Option<Duration>) -> Self {
        self.first_frame = limit.map(|l| (tokio::time::Instant::now() + l, l));
        self
    }

    /// Next frame, `None` at the end of the response.
    pub async fn next(
        &mut self,
        cancel: &CancellationToken,
    ) -> Result<Option<Frame>, ProviderError> {
        loop {
            if let Some(frame) = self.pending.pop_front() {
                self.first_frame = None;
                return Ok(Some(frame));
            }
            if self.done {
                return Ok(None);
            }
            let Some(response) = self.response.as_mut() else {
                return Ok(None);
            };
            if self.format == StreamFormat::None {
                self.first_frame = None;
                let response = self.response.take().expect("response present");
                let data = read_all(response, cancel).await?;
                self.done = true;
                return Ok(Some(Frame { event: None, data }));
            }
            let (wait, stalled) = match self.first_frame {
                Some((deadline, limit)) => (
                    deadline.saturating_duration_since(tokio::time::Instant::now()),
                    not_started(limit),
                ),
                None => (
                    IDLE_TIMEOUT,
                    ProviderError::failed("stream stalled: no data for 300 s"),
                ),
            };
            let chunk = tokio::select! {
                chunk = tokio::time::timeout(wait, response.chunk()) => chunk
                    .map_err(|_| stalled)?
                    .map_err(|e| ProviderError::failed(format!("stream interrupted: {}", e.without_url())))?,
                () = cancel.cancelled() => return Err(ProviderError::cancelled("stream cancelled")),
            };
            let lines = match chunk {
                Some(bytes) => self.lines.push(&bytes),
                None => {
                    self.done = true;
                    self.lines.finish()
                }
            };
            for line in lines {
                self.feed_line(line);
            }
            if self.done {
                if let Some(frame) = self.sse.flush() {
                    self.pending.push_back(frame);
                }
            }
        }
    }

    fn feed_line(&mut self, line: String) {
        match self.format {
            StreamFormat::Ndjson => {
                if !line.trim().is_empty() {
                    self.pending.push_back(Frame {
                        event: None,
                        data: line,
                    });
                }
            }
            StreamFormat::Sse => {
                if let Some(frame) = self.sse.line(&line) {
                    self.pending.push_back(frame);
                }
            }
            StreamFormat::None => {}
        }
    }
}

/// Splits bytes into lines (`\n` or `\r\n`), decoding each complete line
/// as UTF-8 so multi-byte characters split across chunks stay intact.
#[derive(Default)]
pub struct LineSplitter {
    buffer: Vec<u8>,
}

impl LineSplitter {
    pub fn push(&mut self, bytes: &[u8]) -> Vec<String> {
        self.buffer.extend_from_slice(bytes);
        let mut lines = Vec::new();
        while let Some(pos) = self.buffer.iter().position(|b| *b == b'\n') {
            let mut line: Vec<u8> = self.buffer.drain(..=pos).collect();
            line.pop();
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            lines.push(String::from_utf8_lossy(&line).into_owned());
        }
        lines
    }

    pub fn finish(&mut self) -> Vec<String> {
        if self.buffer.is_empty() {
            return Vec::new();
        }
        let rest = std::mem::take(&mut self.buffer);
        vec![String::from_utf8_lossy(&rest)
            .trim_end_matches('\r')
            .to_owned()]
    }
}

/// Server-sent events assembly (`event:`/`data:` fields, blank line ends
/// an event; comments and other fields are ignored).
#[derive(Default)]
pub struct SseState {
    event: Option<String>,
    data: Vec<String>,
}

impl SseState {
    pub fn line(&mut self, line: &str) -> Option<Frame> {
        if line.is_empty() {
            return self.flush();
        }
        if line.starts_with(':') {
            return None;
        }
        let (field, value) = match line.split_once(':') {
            Some((field, value)) => (field, value.strip_prefix(' ').unwrap_or(value)),
            None => (line, ""),
        };
        match field {
            "event" => self.event = Some(value.to_owned()),
            "data" => self.data.push(value.to_owned()),
            _ => {}
        }
        None
    }

    pub fn flush(&mut self) -> Option<Frame> {
        if self.data.is_empty() {
            self.event = None;
            return None;
        }
        Some(Frame {
            event: self.event.take(),
            data: std::mem::take(&mut self.data).join("\n"),
        })
    }
}

pub(crate) async fn read_all(
    response: reqwest::Response,
    cancel: &CancellationToken,
) -> Result<String, ProviderError> {
    tokio::select! {
        text = response.text() => text.map_err(|e| ProviderError::failed(format!("cannot read response: {}", e.without_url()))),
        () = cancel.cancelled() => Err(ProviderError::cancelled("request cancelled")),
    }
}

async fn read_limited(
    response: reqwest::Response,
    cancel: &CancellationToken,
) -> Result<String, ProviderError> {
    let mut stream = response.bytes_stream();
    let mut body = Vec::new();
    loop {
        let next = tokio::select! {
            next = stream.next() => next,
            () = cancel.cancelled() => return Err(ProviderError::cancelled("request cancelled")),
        };
        match next {
            Some(Ok(bytes)) => {
                body.extend_from_slice(&bytes);
                if body.len() >= MAX_ERROR_BODY {
                    break;
                }
            }
            _ => break,
        }
    }
    Ok(String::from_utf8_lossy(&body).into_owned())
}

/// Transport failure. The URL is dropped from the message: a query-string
/// key (generic APIs) must never be echoed.
/// The server took the request but did not start on it within `limit`.
pub fn not_started(limit: Duration) -> ProviderError {
    ProviderError::failed(format!("{NOT_STARTED} em {} s", limit.as_secs()))
}

const NOT_STARTED: &str = "o servidor não começou a responder";

/// The server is overloaded, rate limited, failing or unreachable: worth
/// asking another connection, and better explained to the user.
pub fn is_overload(error: &ProviderError) -> bool {
    let m = error.message.to_ascii_lowercase();
    error.kind != ProviderErrorKind::Cancelled
        && (m.contains(NOT_STARTED)
            || ["(http 408)", "(http 429)", "(http 529)"]
                .iter()
                .any(|s| m.contains(s))
            || (500..600).any(|code| m.contains(&format!("(http {code})")))
            || m.contains("unable to start processing")
            || m.contains("overloaded")
            || m.contains("server is busy")
            || m.starts_with("stream stalled")
            || m.starts_with("cannot reach "))
}

fn transport_error(error: reqwest::Error, url: &str) -> ProviderError {
    let host = url
        .split("://")
        .nth(1)
        .and_then(|rest| rest.split(['/', '?']).next())
        .unwrap_or("?");
    let detail = error.without_url();
    ProviderError::unavailable(format!("cannot reach {host}: {detail}"))
}

/// Maps an HTTP error status plus the API's error body.
pub fn status_error(status: u16, body: &str) -> ProviderError {
    let message = error_message(body);
    // Gemini answers a bad key with 400 + reason API_KEY_INVALID.
    let auth = matches!(status, 401 | 403) || (status == 400 && body.contains("API_KEY_INVALID"));
    let (kind, what) = match status {
        _ if auth => (
            ProviderErrorKind::Unavailable,
            "authentication rejected — check the API key",
        ),
        402 => (
            ProviderErrorKind::Failed,
            "insufficient credit or spending limit reached",
        ),
        404 => (
            ProviderErrorKind::InvalidRequest,
            "not found — check the base URL and the model",
        ),
        400 | 409 | 413 | 422 => (ProviderErrorKind::InvalidRequest, "request rejected"),
        429 => (ProviderErrorKind::Failed, "rate limit or quota exceeded"),
        500..=599 => (ProviderErrorKind::Failed, "server error"),
        _ => (ProviderErrorKind::Failed, "unexpected status"),
    };
    ProviderError::new(kind, format!("{what} (HTTP {status}): {message}"))
}

/// What a 402 says the credit pays for (OpenRouter: "… You requested up
/// to 131072 tokens, but can only afford 42012 …").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Affordable {
    /// Output tokens the request asked for, when the message says.
    pub requested: Option<u32>,
    /// Output tokens the credit still pays for.
    pub tokens: u32,
}

pub fn affordable_output_tokens(error: &ProviderError) -> Option<Affordable> {
    let message = &error.message;
    if !message.contains("(HTTP 402)") {
        return None;
    }
    let number_after = |mark: &str| {
        let rest = &message[message.find(mark)? + mark.len()..];
        let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
        digits.parse::<u32>().ok()
    };
    Some(Affordable {
        requested: number_after("requested up to "),
        tokens: number_after("can only afford ")?,
    })
}

/// Best-effort error text from a JSON (or plain) error body.
pub fn error_message(body: &str) -> String {
    if let Ok(json) = serde_json::from_str::<Value>(body) {
        for path in [
            "error.message",
            "error",
            "message",
            "detail",
            "error_description",
        ] {
            if let Some(text) = jsonpath::get(&json, path).and_then(Value::as_str) {
                return truncate(text, 500);
            }
        }
        return truncate(&json.to_string(), 500);
    }
    let text = body.trim();
    if text.is_empty() {
        "(empty body)".into()
    } else {
        truncate(text, 500)
    }
}

pub fn truncate(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_owned();
    }
    let cut: String = text.chars().take(max_chars).collect();
    format!("{cut}…")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_survive_split_utf8_and_crlf() {
        let mut splitter = LineSplitter::default();
        let text = "olá\r\nmundo\n".as_bytes();
        let (a, b) = text.split_at(3); // splits inside "á"
        assert!(splitter.push(a).is_empty());
        assert_eq!(splitter.push(b), vec!["olá", "mundo"]);
        assert_eq!(splitter.push(b"fim"), Vec::<String>::new());
        assert_eq!(splitter.finish(), vec!["fim"]);
    }

    #[test]
    fn sse_assembles_events() {
        let mut sse = SseState::default();
        let mut frames = Vec::new();
        for line in [
            ": ping",
            "event: message_start",
            "data: {\"a\":1}",
            "",
            "data: linha 1",
            "data: linha 2",
            "",
            "data: [DONE]",
        ] {
            frames.extend(sse.line(line));
        }
        frames.extend(sse.flush());
        assert_eq!(
            frames,
            vec![
                Frame {
                    event: Some("message_start".into()),
                    data: "{\"a\":1}".into()
                },
                Frame {
                    event: None,
                    data: "linha 1\nlinha 2".into()
                },
                Frame {
                    event: None,
                    data: "[DONE]".into()
                },
            ]
        );
    }

    #[test]
    fn errors_carry_the_api_message() {
        let err = status_error(401, r#"{"error":{"message":"Invalid API key"}}"#);
        assert_eq!(err.kind, ProviderErrorKind::Unavailable);
        assert!(err.message.contains("Invalid API key"), "{}", err.message);
        let gemini = r#"{"error":{"code":400,"message":"API key not valid. Please pass a valid API key.","status":"INVALID_ARGUMENT","details":[{"reason":"API_KEY_INVALID"}]}}"#;
        let err = status_error(400, gemini);
        assert_eq!(err.kind, ProviderErrorKind::Unavailable);
        assert!(
            err.message.starts_with("authentication rejected"),
            "{}",
            err.message
        );
        assert_eq!(
            status_error(400, "{}").kind,
            ProviderErrorKind::InvalidRequest
        );
        let err = status_error(429, "slow down");
        assert_eq!(err.kind, ProviderErrorKind::Failed);
        assert!(err.message.contains("slow down"));
        assert_eq!(error_message(r#"{"detail":"x"}"#), "x");
        assert_eq!(error_message(""), "(empty body)");
    }

    #[test]
    fn a_402_tells_what_the_credit_pays_for() {
        let openrouter = r#"{"error":{"message":"This request requires more credits, or fewer max_tokens. You requested up to 131072 tokens, but can only afford 42012. To increase, visit https://openrouter.ai/settings/keys and adjust the key's total limit","code":402}}"#;
        let err = status_error(402, openrouter);
        assert!(
            err.message.starts_with("insufficient credit"),
            "{}",
            err.message
        );
        assert_eq!(
            affordable_output_tokens(&err),
            Some(Affordable {
                requested: Some(131072),
                tokens: 42012
            })
        );
        // A 402 without the number, or the words in another status: nothing.
        assert_eq!(
            affordable_output_tokens(&status_error(402, "Payment required")),
            None
        );
        let other = status_error(400, r#"{"error":{"message":"can only afford 10"}}"#);
        assert_eq!(affordable_output_tokens(&other), None);
    }
}
