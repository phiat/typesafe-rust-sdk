//! Why a call failed.

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;

/// What kind of failure an [`Error`] is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ErrorKind {
    /// No key was given and `TYPESAFE_API_KEY` is unset. No request was made.
    NoApiKey,
    /// The call was malformed, such as a Choice with no options. No request
    /// was made.
    InvalidRequest,
    /// 400
    BadRequest,
    /// 401
    Authentication,
    /// 403
    PermissionDenied,
    /// 404
    NotFound,
    /// 422. The message lists each field as `path: problem`.
    UnprocessableEntity,
    /// 429
    RateLimited,
    /// 529
    Overloaded,
    /// Any other 5xx.
    ServerError,
    /// Any other status.
    HttpError,
    /// No response in time.
    Timeout,
    /// No response: the connection failed or broke.
    Connection,
    /// A 2xx whose body is missing a required field. The message names it.
    InvalidResponse,
}

impl ErrorKind {
    pub(crate) fn for_status(status: u16) -> ErrorKind {
        match status {
            400 => ErrorKind::BadRequest,
            401 => ErrorKind::Authentication,
            403 => ErrorKind::PermissionDenied,
            404 => ErrorKind::NotFound,
            422 => ErrorKind::UnprocessableEntity,
            429 => ErrorKind::RateLimited,
            529 => ErrorKind::Overloaded,
            500..=599 => ErrorKind::ServerError,
            _ => ErrorKind::HttpError,
        }
    }
}

/// Why a call failed.
///
/// Its [`Display`](fmt::Display) reads like `422 questions.tone.criteria: field
/// required (request_id=req_123)`.
#[derive(Clone, Debug)]
pub struct Error {
    kind: ErrorKind,
    status: Option<u16>,
    message: String,
    body: Option<Value>,
    request_id: Option<String>,
    retry_after: Option<Duration>,
    source: Option<Arc<dyn std::error::Error + Send + Sync>>,
}

impl Error {
    pub fn kind(&self) -> ErrorKind {
        self.kind
    }

    /// The HTTP status, when there was a response.
    pub fn status(&self) -> Option<u16> {
        self.status
    }

    /// The server's message, or a description of what went wrong.
    pub fn message(&self) -> &str {
        &self.message
    }

    /// The response body: JSON when it parsed, otherwise the text as a string.
    pub fn body(&self) -> Option<&Value> {
        self.body.as_ref()
    }

    /// The `x-typesafe-request-id` header, when there was a response.
    pub fn request_id(&self) -> Option<&str> {
        self.request_id.as_deref()
    }

    /// How long the server asked to wait before retrying, from
    /// `retry-after-ms` or `retry-after`.
    pub fn retry_after(&self) -> Option<Duration> {
        self.retry_after
    }

    fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Error {
            kind,
            status: None,
            message: message.into(),
            body: None,
            request_id: None,
            retry_after: None,
            source: None,
        }
    }

    pub(crate) fn no_api_key() -> Self {
        Error::new(
            ErrorKind::NoApiKey,
            "no API key: pass one to Config::api_key or set TYPESAFE_API_KEY",
        )
    }

    pub(crate) fn invalid_request(message: impl Into<String>) -> Self {
        Error::new(ErrorKind::InvalidRequest, message)
    }

    pub(crate) fn invalid_response(
        message: impl Into<String>,
        status: u16,
        body: Option<Value>,
        request_id: Option<String>,
    ) -> Self {
        Error {
            status: Some(status),
            body,
            request_id,
            ..Error::new(ErrorKind::InvalidResponse, message)
        }
    }

    pub(crate) fn from_response(
        status: u16,
        body: &[u8],
        request_id: Option<String>,
        retry_after: Option<Duration>,
    ) -> Self {
        let (body, message) = read_error_body(body);
        Error {
            status: Some(status),
            body,
            request_id,
            retry_after,
            ..Error::new(ErrorKind::for_status(status), message)
        }
    }

    pub(crate) fn from_reqwest(error: reqwest::Error) -> Self {
        let kind = if error.is_timeout() {
            ErrorKind::Timeout
        } else if error.is_builder() {
            ErrorKind::InvalidRequest
        } else {
            ErrorKind::Connection
        };
        Error {
            source: Some(Arc::new(error.without_url())),
            ..Error::new(kind, describe_reqwest(kind))
        }
    }
}

fn describe_reqwest(kind: ErrorKind) -> &'static str {
    match kind {
        ErrorKind::Timeout => "the request timed out",
        ErrorKind::InvalidRequest => "the request could not be built",
        _ => "the connection failed",
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(status) = self.status {
            write!(f, "{status} ")?;
        }
        f.write_str(&self.message)?;
        if let Some(source) = &self.source {
            write!(f, ": {source}")?;
        }
        if let Some(id) = &self.request_id {
            write!(f, " (request_id={id})")?;
        }
        Ok(())
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source
            .as_deref()
            .map(|e| e as &(dyn std::error::Error + 'static))
    }
}

const MAX_BODY_IN_MESSAGE: usize = 200;

/// The body as JSON (or text), and the message to show for it.
fn read_error_body(bytes: &[u8]) -> (Option<Value>, String) {
    let text = String::from_utf8_lossy(bytes);
    if text.trim().is_empty() {
        return (None, "(no body)".into());
    }
    match serde_json::from_slice::<Value>(bytes) {
        Ok(json) => {
            let message = extract_message(&json).unwrap_or_else(|| truncate(&json.to_string()));
            (Some(json), message)
        }
        Err(_) => (Some(Value::String(text.to_string())), truncate(text.trim())),
    }
}

// The message shapes the official SDKs read: a string error, an error or
// detail object with a message, or a validation list of {loc, msg}.
fn extract_message(body: &Value) -> Option<String> {
    let str_at = |value: Option<&Value>| value.and_then(Value::as_str).map(str::to_string);
    if let Value::String(s) = body {
        return (!s.is_empty()).then(|| s.clone());
    }
    let error = body.get("error");
    let detail = body.get("detail");
    str_at(error)
        .or_else(|| str_at(error.and_then(|e| e.get("message"))))
        .or_else(|| str_at(body.get("message")))
        .or_else(|| str_at(detail))
        .or_else(|| str_at(detail.and_then(|d| d.get("message"))))
        .or_else(|| {
            detail
                .and_then(Value::as_array)
                .and_then(|d| validation_message(d))
        })
}

fn validation_message(entries: &[Value]) -> Option<String> {
    let parts: Vec<String> = entries
        .iter()
        .filter_map(|entry| {
            let msg = entry.get("msg")?.as_str()?;
            let path = match entry.get("loc") {
                Some(Value::Array(loc)) => loc
                    .iter()
                    .filter(|part| part.as_str() != Some("body"))
                    .map(|part| match part {
                        Value::String(s) => s.clone(),
                        other => other.to_string(),
                    })
                    .collect::<Vec<_>>()
                    .join("."),
                Some(Value::String(s)) if s != "body" => s.clone(),
                _ => String::new(),
            };
            Some(if path.is_empty() {
                msg.to_string()
            } else {
                format!("{path}: {msg}")
            })
        })
        .collect();
    (!parts.is_empty()).then(|| parts.join("; "))
}

fn truncate(raw: &str) -> String {
    match raw.char_indices().nth(MAX_BODY_IN_MESSAGE) {
        Some((cut, _)) => format!("{}…", &raw[..cut]),
        None => raw.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn message(body: Value) -> String {
        Error::from_response(400, body.to_string().as_bytes(), None, None).message
    }

    #[test]
    fn maps_statuses() {
        let kinds: Vec<ErrorKind> = [400, 401, 403, 404, 422, 429, 529, 500, 503, 418]
            .into_iter()
            .map(ErrorKind::for_status)
            .collect();
        assert_eq!(
            kinds,
            [
                ErrorKind::BadRequest,
                ErrorKind::Authentication,
                ErrorKind::PermissionDenied,
                ErrorKind::NotFound,
                ErrorKind::UnprocessableEntity,
                ErrorKind::RateLimited,
                ErrorKind::Overloaded,
                ErrorKind::ServerError,
                ErrorKind::ServerError,
                ErrorKind::HttpError,
            ]
        );
    }

    #[test]
    fn reads_the_message_shapes() {
        assert_eq!(message(json!({"error": "bad key"})), "bad key");
        assert_eq!(message(json!({"error": {"message": "nested"}})), "nested");
        assert_eq!(message(json!({"message": "plain"})), "plain");
        assert_eq!(message(json!({"detail": "detailed"})), "detailed");
        assert_eq!(message(json!({"detail": {"message": "deep"}})), "deep");
        assert_eq!(
            message(json!({"detail": [
                {"loc": ["body", "questions", "tone", "criteria"], "msg": "field required"},
                {"loc": ["body", "state"], "msg": "must not be empty"},
                {"loc": [], "msg": "and one more"}
            ]})),
            "questions.tone.criteria: field required; state: must not be empty; and one more"
        );
        assert_eq!(message(json!({"other": 1})), r#"{"other":1}"#);
    }

    #[test]
    fn falls_back_to_the_body() {
        let error = Error::from_response(502, b"<html>Bad gateway</html>", None, None);
        assert_eq!(error.message(), "<html>Bad gateway</html>");
        assert_eq!(error.body(), Some(&json!("<html>Bad gateway</html>")));

        let empty = Error::from_response(503, b"", None, None);
        assert_eq!(empty.message(), "(no body)");
        assert_eq!(empty.body(), None);

        let long = "x".repeat(300);
        let error = Error::from_response(500, long.as_bytes(), None, None);
        assert_eq!(error.message().chars().count(), MAX_BODY_IN_MESSAGE + 1);
        assert!(error.message().ends_with('…'));
    }

    #[test]
    fn displays_status_and_request_id() {
        let error = Error::from_response(
            422,
            br#"{"detail": "bad"}"#,
            Some("req_9".into()),
            Some(Duration::from_millis(5)),
        );
        assert_eq!(error.to_string(), "422 bad (request_id=req_9)");
        assert_eq!(error.kind(), ErrorKind::UnprocessableEntity);
        assert_eq!(error.retry_after(), Some(Duration::from_millis(5)));
        assert_eq!(Error::no_api_key().status(), None);
    }
}
