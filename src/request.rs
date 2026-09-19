//! Building requests and reading replies, shared by the async and blocking
//! clients. Each client only sends attempts and sleeps between them.

use std::time::{Duration, Instant};

use reqwest::Method;
use reqwest::header::{self, HeaderMap, HeaderName, HeaderValue};
use serde::Serialize;
use serde_json::value::RawValue;
use serde_json::{Map, Value};

use crate::config::{CallOptions, Config};
use crate::error::{Error, ErrorKind};
use crate::question::Questions;
use crate::response::{self, Model, Response};
use crate::retry::{self, Failure, RetryPolicy};

const SDK: &str = concat!("typesafe-rust-sdk/", env!("CARGO_PKG_VERSION"));

/// A request ready to send, retries included.
pub(crate) struct Prepared {
    pub method: Method,
    pub url: String,
    pub headers: HeaderMap,
    pub body: Option<Vec<u8>>,
    pub timeout: Duration,
    pub retry: RetryPolicy,
}

/// One attempt's HTTP response.
pub(crate) struct Reply {
    pub status: u16,
    pub headers: HeaderMap,
    pub body: Vec<u8>,
}

pub(crate) fn system_one<S: Serialize + ?Sized>(
    config: &Config,
    state: &S,
    questions: &Questions,
    options: &CallOptions,
) -> Result<Prepared, Error> {
    questions.validate().map_err(Error::invalid_request)?;
    let state = encode_state(state)?;

    #[derive(Serialize)]
    struct Body<'a> {
        state: &'a RawValue,
        model: &'a str,
        questions: &'a Questions,
        #[serde(flatten)]
        extra: Map<String, Value>,
    }
    let mut extra = options.extra_body.clone();
    for reserved in ["state", "model", "questions"] {
        extra.remove(reserved);
    }
    let body = Body {
        state: &state,
        model: options.model.as_deref().unwrap_or(&config.model),
        questions,
        extra,
    };
    let body = serde_json::to_vec(&body)
        .map_err(|e| Error::invalid_request(format!("the request could not be encoded: {e}")))?;
    prepare(
        config,
        Some(options),
        Method::POST,
        "/v1/systemone",
        Some(body),
    )
}

pub(crate) fn models(config: &Config) -> Result<Prepared, Error> {
    prepare(config, None, Method::GET, "/v1/models", None)
}

/// State is sent exactly as serde writes it, so a struct keeps its field order.
fn encode_state<S: Serialize + ?Sized>(state: &S) -> Result<Box<RawValue>, Error> {
    let invalid = || Error::invalid_request("state must be a non-empty string, object or array");
    let json = serde_json::to_string(state)
        .map_err(|e| Error::invalid_request(format!("state could not be encoded: {e}")))?;
    match json.as_bytes().first() {
        Some(b'{' | b'[') => {}
        Some(b'"') => {
            let text: String = serde_json::from_str(&json).map_err(|_| invalid())?;
            if text.trim().is_empty() {
                return Err(invalid());
            }
        }
        _ => return Err(invalid()),
    }
    RawValue::from_string(json).map_err(|_| invalid())
}

fn prepare(
    config: &Config,
    options: Option<&CallOptions>,
    method: Method,
    path: &str,
    body: Option<Vec<u8>>,
) -> Result<Prepared, Error> {
    let api_key = config.api_key.as_deref().ok_or_else(Error::no_api_key)?;
    let timeout = options.and_then(|o| o.timeout).unwrap_or(config.timeout);
    if timeout.is_zero() {
        return Err(Error::invalid_request("timeout must be greater than zero"));
    }

    let mut headers = HeaderMap::new();
    let runtime = format!(
        "rust ({}; {})",
        std::env::consts::OS,
        std::env::consts::ARCH
    );
    let defaults = [
        ("accept", "application/json"),
        ("user-agent", SDK),
        ("x-typesafe-sdk", SDK),
        ("x-typesafe-runtime", &runtime),
    ];
    let content_type = body.as_ref().map(|_| ("content-type", "application/json"));
    let extra = config
        .headers
        .iter()
        .chain(options.map_or(&[][..], |o| &o.headers));
    for (name, value) in defaults
        .into_iter()
        .chain(content_type)
        .chain(extra.map(|(n, v)| (n.as_str(), v.as_str())))
    {
        let name = HeaderName::from_bytes(name.as_bytes())
            .map_err(|_| Error::invalid_request(format!("invalid header name {name:?}")))?;
        let value = HeaderValue::from_str(value)
            .map_err(|_| Error::invalid_request(format!("invalid value for header {name}")))?;
        headers.insert(name, value);
    }
    let mut auth = HeaderValue::from_str(&format!("Bearer {api_key}"))
        .map_err(|_| Error::invalid_request("the API key is not a valid header value"))?;
    auth.set_sensitive(true);
    headers.insert(header::AUTHORIZATION, auth);

    Ok(Prepared {
        method,
        url: format!("{}{path}", config.base_url),
        headers,
        body,
        timeout,
        retry: options
            .and_then(|o| o.retry.clone())
            .unwrap_or_else(|| config.retry.clone()),
    })
}

/// Tracks a call's retries across attempts.
pub(crate) struct Retries {
    policy: RetryPolicy,
    deadline: Option<Instant>,
    pub attempts: u32,
}

impl Retries {
    pub fn start(policy: RetryPolicy) -> Self {
        let deadline = policy.budget.map(|budget| Instant::now() + budget);
        Retries {
            policy,
            deadline,
            attempts: 0,
        }
    }

    /// The header to send on this attempt, if it's a retry.
    pub fn retry_count_header(&self) -> Option<(&'static str, String)> {
        (self.attempts > 1).then(|| ("x-typesafe-retry-count", (self.attempts - 1).to_string()))
    }

    /// How long to wait before trying again, or `None` to stop here.
    pub fn after(&self, result: &Result<Reply, Error>) -> Option<Duration> {
        let failure = match result {
            Ok(reply) if (200..300).contains(&reply.status) => return None,
            Ok(reply) => Failure::Status {
                status: reply.status,
                retry_after: retry::retry_after(&reply.headers),
            },
            Err(e) if matches!(e.kind(), ErrorKind::Timeout | ErrorKind::Connection) => {
                Failure::Transport
            }
            Err(_) => return None,
        };
        let remaining = self
            .deadline
            .map(|deadline| deadline.saturating_duration_since(Instant::now()));
        self.policy.next_delay(self.attempts, failure, remaining)
    }
}

/// The JSON object body of a 2xx reply, with its request id and status.
pub(crate) struct Success {
    pub body: Value,
    pub request_id: Option<String>,
    pub status: u16,
}

pub(crate) fn finish(result: Result<Reply, Error>) -> Result<Success, Error> {
    let reply = result?;
    let request_id = reply
        .headers
        .get("x-typesafe-request-id")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    if !(200..300).contains(&reply.status) {
        return Err(Error::from_response(
            reply.status,
            &reply.body,
            request_id,
            retry::retry_after(&reply.headers),
        ));
    }
    match serde_json::from_slice::<Value>(&reply.body) {
        Ok(body @ Value::Object(_)) => Ok(Success {
            body,
            request_id,
            status: reply.status,
        }),
        other => Err(Error::invalid_response(
            "expected a JSON object body",
            reply.status,
            Some(other.unwrap_or_else(|_| {
                Value::String(String::from_utf8_lossy(&reply.body).into_owned())
            })),
            request_id,
        )),
    }
}

pub(crate) fn decode_system_one(
    success: Success,
    questions: &Questions,
) -> Result<Response, Error> {
    response::decode(&success.body, questions, success.request_id.clone())
        .map_err(|field| invalid_data(field, success))
}

pub(crate) fn decode_models(success: Success) -> Result<Vec<Model>, Error> {
    response::decode_models(&success.body).map_err(|field| invalid_data(field, success))
}

fn invalid_data(field: String, success: Success) -> Error {
    Error::invalid_response(
        format!("invalid response data at {field:?}"),
        success.status,
        Some(success.body),
        success.request_id,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Noul;
    use serde_json::json;

    fn config() -> Config {
        Config::new()
            .api_key("sk-test")
            .base_url("http://localhost:1")
    }

    fn questions() -> Questions {
        Questions::new().ask("q", Noul::new("Is it?"))
    }

    fn body(prepared: &Prepared) -> Value {
        serde_json::from_slice(prepared.body.as_ref().unwrap()).unwrap()
    }

    #[test]
    fn builds_the_body_and_headers() {
        let options = CallOptions::new()
            .model("jev-1.13.0")
            .extra_body("trace", true)
            .extra_body("model", "sneaky")
            .header("X-Custom", "1");
        let prepared =
            system_one(&config(), &json!({"ticket": "hi"}), &questions(), &options).unwrap();
        assert_eq!(prepared.url, "http://localhost:1/v1/systemone");
        assert_eq!(
            body(&prepared),
            json!({
                "state": {"ticket": "hi"},
                "model": "jev-1.13.0",
                "questions": {"q": {"type": "noul", "instructions": "Is it?"}},
                "trace": true
            })
        );
        let h = &prepared.headers;
        assert_eq!(h["authorization"], "Bearer sk-test");
        assert!(h["authorization"].is_sensitive());
        assert_eq!(h["content-type"], "application/json");
        assert_eq!(h["x-custom"], "1");
        assert!(
            h["user-agent"]
                .to_str()
                .unwrap()
                .starts_with("typesafe-rust-sdk/")
        );
        assert!(
            h["x-typesafe-runtime"]
                .to_str()
                .unwrap()
                .starts_with("rust (")
        );
    }

    #[test]
    fn keeps_struct_field_order_in_state() {
        #[derive(Serialize)]
        struct Ticket {
            subject: &'static str,
            body: &'static str,
        }
        let state = Ticket {
            subject: "Refund",
            body: "Charged twice",
        };
        let prepared = system_one(&config(), &state, &questions(), &CallOptions::new()).unwrap();
        let raw = String::from_utf8(prepared.body.unwrap()).unwrap();
        assert!(raw.starts_with(r#"{"state":{"subject":"Refund","body":"Charged twice"},"#));
    }

    #[test]
    fn rejects_bad_calls_before_sending() {
        let kind = |result: Result<Prepared, Error>| result.err().unwrap().kind();
        let options = CallOptions::new();
        assert_eq!(
            kind(system_one(&config(), "", &questions(), &options)),
            ErrorKind::InvalidRequest
        );
        assert_eq!(
            kind(system_one(&config(), &Value::Null, &questions(), &options)),
            ErrorKind::InvalidRequest
        );
        assert_eq!(
            kind(system_one(&config(), &42, &questions(), &options)),
            ErrorKind::InvalidRequest
        );
        assert_eq!(
            kind(system_one(&config(), "text", &Questions::new(), &options)),
            ErrorKind::InvalidRequest
        );
        assert_eq!(
            kind(system_one(&Config::new(), "text", &questions(), &options)),
            ErrorKind::NoApiKey
        );
        assert_eq!(
            kind(system_one(
                &config(),
                "text",
                &questions(),
                &CallOptions::new().timeout(Duration::ZERO)
            )),
            ErrorKind::InvalidRequest
        );
        assert_eq!(
            kind(system_one(
                &config(),
                "text",
                &questions(),
                &CallOptions::new().header("bad header", "x")
            )),
            ErrorKind::InvalidRequest
        );
    }

    #[test]
    fn finish_reads_replies() {
        let reply = |status: u16, body: &str| Reply {
            status,
            headers: HeaderMap::from_iter([(
                HeaderName::from_static("x-typesafe-request-id"),
                HeaderValue::from_static("req_1"),
            )]),
            body: body.as_bytes().to_vec(),
        };
        let ok = finish(Ok(reply(200, r#"{"a": 1}"#))).ok().unwrap();
        assert_eq!(ok.body, json!({"a": 1}));
        assert_eq!(ok.request_id.as_deref(), Some("req_1"));

        let not_object = finish(Ok(reply(200, "[1]"))).err().unwrap();
        assert_eq!(not_object.kind(), ErrorKind::InvalidResponse);
        let not_json = finish(Ok(reply(200, "hello"))).err().unwrap();
        assert_eq!(not_json.body(), Some(&json!("hello")));

        let overloaded = finish(Ok(reply(529, r#"{"error": "busy"}"#)))
            .err()
            .unwrap();
        assert_eq!(overloaded.kind(), ErrorKind::Overloaded);
        assert_eq!(overloaded.request_id(), Some("req_1"));
        assert_eq!(overloaded.message(), "busy");
    }
}
