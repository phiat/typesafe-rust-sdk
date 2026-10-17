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
use crate::image::{self, Image};
use crate::question::Questions;
use crate::response::{self, Model, Response};
use crate::retry::{self, Failure, RetryPolicy};

const SDK: &str = concat!("typesafe-rust-sdk/", env!("CARGO_PKG_VERSION"));

/// Images are experimental, and the API refuses them without this header.
const IMAGES_HEADER: &str = "typesafe-danger-zone-images";

/// A request body, its content type and, if compressed, its content encoding.
struct Payload {
    content_type: String,
    content_encoding: Option<&'static str>,
    bytes: Vec<u8>,
}

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
    let encoded = |e: serde_json::Error| {
        Error::invalid_request(format!("the request could not be encoded: {e}"))
    };
    let questions_json = serde_json::value::to_raw_value(questions).map_err(encoded)?;
    let images = check_images(options, &state, &questions_json, questions.has_box())?;

    #[derive(Serialize)]
    struct Body<'a> {
        state: &'a RawValue,
        model: &'a str,
        questions: &'a RawValue,
        #[serde(skip_serializing_if = "Option::is_none")]
        assets: Option<Map<String, Value>>,
        #[serde(flatten)]
        extra: Map<String, Value>,
    }
    let mut extra = options.extra_body.clone();
    for reserved in ["state", "model", "questions"] {
        extra.remove(reserved);
    }
    // Without attached images, "assets" stays available as a raw escape hatch.
    if !images.is_empty() {
        extra.remove("assets");
    }
    let assets = (!images.is_empty()).then(|| {
        images
            .iter()
            .enumerate()
            .map(|(i, image)| {
                let asset = serde_json::json!({"type": "image", "part": part_name(i)});
                (image.reference().to_string(), asset)
            })
            .collect()
    });
    let body = Body {
        state: &state,
        model: options.model.as_deref().unwrap_or(&config.model),
        questions: &questions_json,
        assets,
        extra,
    };
    let json = serde_json::to_vec(&body).map_err(encoded)?;
    let payload = if images.is_empty() {
        Payload {
            content_type: "application/json".into(),
            content_encoding: None,
            bytes: json,
        }
    } else {
        multipart(json, &images)
    };
    let compression = options.compression.unwrap_or(config.compression);
    let (bytes, content_encoding) = compression.encode(payload.bytes)?;
    let payload = Payload {
        content_encoding,
        bytes,
        ..payload
    };
    prepare(
        config,
        Some(options),
        Method::POST,
        "/v1/systemone",
        Some(payload),
        !images.is_empty(),
    )
}

pub(crate) fn models(config: &Config) -> Result<Prepared, Error> {
    prepare(config, None, Method::GET, "/v1/models", None, false)
}

/// The images to send, once each, in the order they were attached.
///
/// The API treats a reference it was not given as plain text, and answers a
/// Box question about no image with a box all the same. Both would be silent
/// wrong answers, so the problems it can't see are reported here.
fn check_images<'a>(
    options: &'a CallOptions,
    state: &RawValue,
    questions: &RawValue,
    has_box: bool,
) -> Result<Vec<&'a Image>, Error> {
    let mut images: Vec<&Image> = Vec::new();
    for image in &options.images {
        if !images
            .iter()
            .any(|seen| seen.reference() == image.reference())
        {
            images.push(image);
        }
    }
    let unattached = image::find_unattached(state.get(), &images)
        .or_else(|| image::find_unattached(questions.get(), &images));
    if let Some(reference) = unattached {
        return Err(Error::invalid_request(format!(
            "{reference} refers to an image that is not attached: add it with CallOptions::image"
        )));
    }
    if images.is_empty() && has_box {
        return Err(Error::invalid_request(
            "a Box question needs an image: attach one with CallOptions::image",
        ));
    }
    for image in &images {
        let reference = image.reference();
        if !state.get().contains(reference) && !questions.get().contains(reference) {
            return Err(Error::invalid_request(format!(
                "image {reference} is attached but nothing in the state or questions refers to it"
            )));
        }
    }
    Ok(images)
}

fn part_name(index: usize) -> String {
    format!("image{index}")
}

/// A `multipart/form-data` body: the request JSON first, then one file part per
/// image. The API recommends it over base64, which makes images a third larger.
fn multipart(json: Vec<u8>, images: &[&Image]) -> Payload {
    let boundary = format!("typesafe-{:032x}", fastrand::u128(..));
    let mut bytes = Vec::with_capacity(
        json.len() + images.iter().map(|i| i.bytes().len() + 200).sum::<usize>(),
    );
    let head = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"request\"\r\n\
         Content-Type: application/json\r\n\r\n"
    );
    bytes.extend_from_slice(head.as_bytes());
    bytes.extend_from_slice(&json);
    for (i, image) in images.iter().enumerate() {
        let name = part_name(i);
        let head = format!(
            "\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"; \
             filename=\"{name}.{}\"\r\nContent-Type: {}\r\n\r\n",
            image.format().extension(),
            image.format().content_type()
        );
        bytes.extend_from_slice(head.as_bytes());
        bytes.extend_from_slice(image.bytes());
    }
    bytes.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    Payload {
        content_type: format!("multipart/form-data; boundary={boundary}"),
        content_encoding: None,
        bytes,
    }
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
    body: Option<Payload>,
    images: bool,
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
    let content_type = body
        .as_ref()
        .map(|payload| ("content-type", payload.content_type.as_str()));
    let content_encoding = body
        .as_ref()
        .and_then(|payload| payload.content_encoding)
        .map(|encoding| ("content-encoding", encoding));
    let images = images.then_some((IMAGES_HEADER, "true"));
    let extra = config
        .headers
        .iter()
        .chain(options.map_or(&[][..], |o| &o.headers));
    for (name, value) in defaults
        .into_iter()
        .chain(content_type)
        .chain(content_encoding)
        .chain(images)
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
        body: body.map(|payload| payload.bytes),
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
    use crate::{BoxQuestion, Noul};
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
            .model("jev-1.14.0")
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
                "model": "jev-1.14.0",
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

    fn photo() -> Image {
        Image::from_bytes(b"\x89PNG\r\n\x1a\nnot really pixels".to_vec()).unwrap()
    }

    fn contains(haystack: &[u8], needle: &[u8]) -> bool {
        haystack
            .windows(needle.len())
            .any(|window| window == needle)
    }

    #[test]
    fn sends_images_as_multipart() {
        let (mug, badge) = (photo(), photo());
        let questions = Questions::new().ask(
            "mug",
            BoxQuestion::new(format!("The mug in `photo`, or the person in {badge}")),
        );
        let state = json!({"photo": mug});
        let options = CallOptions::new()
            .model("jev-1.14.0")
            .image(&mug)
            .image(&badge);
        let prepared = system_one(&config(), &state, &questions, &options).unwrap();

        let h = &prepared.headers;
        assert_eq!(h["typesafe-danger-zone-images"], "true");
        let boundary = h["content-type"]
            .to_str()
            .unwrap()
            .strip_prefix("multipart/form-data; boundary=")
            .unwrap()
            .to_string();
        let raw = prepared.body.unwrap();
        let text = String::from_utf8_lossy(&raw);

        // The request JSON is the first part, and names each image's part.
        let opening = format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"request\"\r\n\
             Content-Type: application/json\r\n\r\n"
        );
        let json = text.strip_prefix(&opening).unwrap();
        let json = &json[..json.find(&format!("\r\n--{boundary}")).unwrap()];
        let request: Value = serde_json::from_str(json).unwrap();
        assert_eq!(request["model"], "jev-1.14.0");
        assert_eq!(request["questions"]["mug"]["type"], "box");
        assert_eq!(
            request["assets"],
            json!({
                mug.reference(): {"type": "image", "part": "image0"},
                badge.reference(): {"type": "image", "part": "image1"},
            })
        );

        // Each image follows as a file part, bytes untouched.
        for (part, ext) in [("image0", "png"), ("image1", "png")] {
            let head = format!(
                "\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"{part}\"; \
                 filename=\"{part}.{ext}\"\r\nContent-Type: image/png\r\n\r\n\
                 \u{fffd}PNG\r\n\u{1a}\nnot really pixels"
            );
            assert!(text.contains(&head), "{text}");
        }
        assert!(contains(&raw, b"\x89PNG\r\n\x1a\nnot really pixels\r\n--"));
        assert!(text.ends_with(&format!("\r\n--{boundary}--\r\n")));
    }

    #[test]
    fn an_image_attached_twice_is_sent_once() {
        let mug = photo();
        let questions = Questions::new().ask("q", Noul::new(format!("Is {mug} a mug?")));
        let options = CallOptions::new().image(&mug).image(&mug.clone());
        let prepared = system_one(&config(), "state", &questions, &options).unwrap();
        let text = String::from_utf8_lossy(prepared.body.as_ref().unwrap()).into_owned();
        assert_eq!(text.matches("form-data; name=\"image").count(), 1);
    }

    #[test]
    fn plain_calls_carry_no_image_header() {
        let prepared = system_one(&config(), "text", &questions(), &CallOptions::new()).unwrap();
        assert!(!prepared.headers.contains_key("typesafe-danger-zone-images"));
        assert_eq!(prepared.headers["content-type"], "application/json");
        assert!(body(&prepared).get("assets").is_none());
    }

    #[test]
    fn image_mistakes_are_caught_before_sending() {
        let problem = |state: Value, questions: Questions, options: CallOptions| {
            let error = system_one(&config(), &state, &questions, &options)
                .err()
                .unwrap();
            assert_eq!(error.kind(), ErrorKind::InvalidRequest);
            error.message().to_string()
        };
        let mug = photo();
        let other = photo();
        let mug_box = || Questions::new().ask("mug", BoxQuestion::new("The mug in `photo`"));

        // A Box question about no image gets a junk box from the API.
        assert!(
            problem(json!({"photo": "x"}), mug_box(), CallOptions::new())
                .contains("needs an image")
        );
        // An attached image nothing refers to.
        assert!(
            problem(
                json!({"photo": "x"}),
                mug_box(),
                CallOptions::new().image(&mug)
            )
            .contains("nothing in the state or questions refers to it")
        );
        // A reference whose image was never attached would be read as text.
        let message = problem(json!({"photo": mug}), mug_box(), CallOptions::new());
        assert!(
            message.contains(mug.reference()) && message.contains("not attached"),
            "{message}"
        );
        let message = problem(
            json!({"photo": mug, "badge": other}),
            mug_box(),
            CallOptions::new().image(&mug),
        );
        assert!(message.contains(other.reference()), "{message}");
    }

    #[test]
    fn extra_body_assets_pass_through_without_attached_images() {
        let options = CallOptions::new().extra_body("assets", json!({"raw": 1}));
        let prepared = system_one(&config(), "text", &questions(), &options).unwrap();
        assert_eq!(body(&prepared)["assets"], json!({"raw": 1}));
    }

    fn big_state() -> Value {
        json!({"log": "the customer repeats themselves. ".repeat(100)})
    }

    /// The request body as JSON, once undone from `encoding`.
    #[cfg(any(feature = "gzip", feature = "zstd"))]
    fn undo(prepared: &Prepared, encoding: &str) -> Value {
        let raw = prepared.body.as_ref().unwrap();
        let json = match encoding {
            #[cfg(feature = "gzip")]
            "gzip" => {
                let mut out = Vec::new();
                std::io::Read::read_to_end(&mut flate2::read::GzDecoder::new(&raw[..]), &mut out)
                    .unwrap();
                out
            }
            #[cfg(feature = "zstd")]
            "zstd" => zstd::decode_all(&raw[..]).unwrap(),
            other => panic!("no decoder for {other}"),
        };
        serde_json::from_slice(&json).unwrap()
    }

    #[test]
    fn compression_is_off_unless_asked_for() {
        let prepared =
            system_one(&config(), &big_state(), &questions(), &CallOptions::new()).unwrap();
        assert!(!prepared.headers.contains_key("content-encoding"));
        assert_eq!(body(&prepared)["state"], big_state());
    }

    #[cfg(any(feature = "gzip", feature = "zstd"))]
    #[test]
    #[allow(clippy::single_element_loop)] // one codec when only one feature is on
    fn compressed_bodies_say_so_and_decode_to_the_same_request() {
        for (codec, name) in [
            #[cfg(feature = "gzip")]
            (crate::Compression::Gzip, "gzip"),
            #[cfg(feature = "zstd")]
            (crate::Compression::Zstd, "zstd"),
        ] {
            let config = config().compression(codec);
            let plain = system_one(
                &self::config(),
                &big_state(),
                &questions(),
                &CallOptions::new(),
            )
            .unwrap();
            let prepared =
                system_one(&config, &big_state(), &questions(), &CallOptions::new()).unwrap();

            assert_eq!(prepared.headers["content-encoding"], name);
            assert_eq!(prepared.headers["content-type"], "application/json");
            assert!(prepared.body.as_ref().unwrap().len() < plain.body.as_ref().unwrap().len() / 5);
            assert_eq!(undo(&prepared, name), body(&plain));

            // A small body is not worth compressing.
            let small = system_one(&config, "short", &questions(), &CallOptions::new()).unwrap();
            assert!(!small.headers.contains_key("content-encoding"));
            assert_eq!(body(&small)["state"], "short");

            // A call can turn it off, or pick another codec over the client's.
            let off = CallOptions::new().compression(crate::Compression::Off);
            let prepared = system_one(&config, &big_state(), &questions(), &off).unwrap();
            assert!(!prepared.headers.contains_key("content-encoding"));
        }
    }

    #[cfg(feature = "gzip")]
    #[test]
    fn a_call_can_turn_compression_on() {
        let options = CallOptions::new().compression(crate::Compression::Gzip);
        let prepared = system_one(&config(), &big_state(), &questions(), &options).unwrap();
        assert_eq!(prepared.headers["content-encoding"], "gzip");
    }

    #[cfg(feature = "gzip")]
    #[test]
    fn a_multipart_body_is_compressed_whole() {
        let mug =
            Image::from_bytes([&b"\x89PNG\r\n\x1a\n"[..], &[7u8; 4000][..]].concat()).unwrap();
        let questions = Questions::new().ask("mug", BoxQuestion::new("The mug in `photo`"));
        let options = CallOptions::new()
            .image(&mug)
            .compression(crate::Compression::Gzip);
        let prepared = system_one(&config(), &json!({"photo": mug}), &questions, &options).unwrap();

        // The content type still names the multipart boundary, and the bytes
        // inside the encoding are the multipart body.
        assert_eq!(prepared.headers["content-encoding"], "gzip");
        assert!(
            prepared.headers["content-type"]
                .to_str()
                .unwrap()
                .starts_with("multipart/form-data; boundary=")
        );
        assert_eq!(prepared.headers["typesafe-danger-zone-images"], "true");
        let mut out = Vec::new();
        let raw = prepared.body.as_ref().unwrap();
        std::io::Read::read_to_end(&mut flate2::read::GzDecoder::new(&raw[..]), &mut out).unwrap();
        assert!(contains(&out, &[7u8; 4000]));
        assert!(raw.len() < out.len() / 4);
    }

    #[test]
    fn long_context_sets_its_header() {
        let options = CallOptions::new().long_context();
        let prepared = system_one(&config(), "text", &questions(), &options).unwrap();
        assert_eq!(
            prepared.headers["typesafe-danger-zone-long-context"],
            "true"
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
