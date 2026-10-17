//! The blocking client against a local mock server.

#![cfg(feature = "blocking")]

mod common;

use std::net::TcpListener;
use std::time::Duration;

use mockito::{Matcher, Server};
use serde_json::json;
use typesafe::blocking::Client;
use typesafe::{CallOptions, Config, ErrorKind, Noul, Questions, RetryPolicy};

use common::*;

fn client(server: &Server) -> Client {
    Client::with_config(config(&server.url())).unwrap()
}

#[test]
fn answers_all_three_types() {
    let mut server = Server::new();
    let mock = server
        .mock("POST", "/v1/systemone")
        .match_header("authorization", "Bearer sk-test")
        .match_header("content-type", "application/json")
        .match_header("x-typesafe-sdk", Matcher::Regex("^typesafe-rust-sdk/".into()))
        .match_header("x-typesafe-retry-count", Matcher::Missing)
        .match_body(Matcher::AllOf(vec![
            Matcher::PartialJson(json!({
                "state": ticket(),
                "model": "jev-latest",
                "questions": {"billing": {"type": "noul", "instructions": "Is this ticket about billing?"}}
            })),
            // Options keep their order on the wire.
            Matcher::Regex(r#""criteria":\{"calm":null,"frustrated":null,"angry":null\}"#.into()),
        ]))
        .with_header("content-type", "application/json")
        .with_header("x-typesafe-request-id", "req_123")
        .with_body(answers().to_string())
        .create();

    let response = client(&server).system_one(&ticket(), &questions()).unwrap();
    mock.assert();

    assert_eq!(response.model, "jev-1.13.0");
    assert_eq!(response.request_id.as_deref(), Some("req_123"));
    assert_eq!(response.usage.input_tokens, 367);
    assert!(response.noul("billing").unwrap().yes());
    let tone = response.choice("tone").unwrap();
    assert!(tone.is("frustrated"));
    assert_eq!(tone.ranked()[0], ("frustrated", 0.86));
    assert_eq!(response.score("urgency").unwrap().level(), 2);
}

#[test]
fn locates_something_in_an_image() {
    let mut server = Server::new();
    let mock = server
        .mock("POST", "/v1/systemone")
        .match_header("typesafe-danger-zone-images", "true")
        .match_header(
            "content-type",
            Matcher::Regex("^multipart/form-data; boundary=typesafe-[0-9a-f]{32}$".into()),
        )
        // The request JSON, then the image as a file part.
        .match_body(Matcher::Regex(
            r#"(?s)name="request".*"questions":\{.*"type":"box".*name="image0"; filename="image0.png""#
                .into(),
        ))
        .with_body(rectangle_answers().to_string())
        .create();

    let photo = red_rectangle();
    let response = client(&server)
        .system_one_with(
            &json!({"photo": photo}),
            &rectangle_questions(),
            &CallOptions::new().model("jev-1.14.0").image(&photo),
        )
        .unwrap();
    mock.assert();

    assert_eq!(response.model, "jev-1.14.0");
    assert!(response.noul("present").unwrap().yes());
    let rect = response.bounding_box("rect").unwrap();
    assert_eq!((rect.left, rect.right), (0.5, 0.91));
    assert_eq!(rect.confidence, Some(0.95));
    assert!(response.bounding_box("present").is_none());
}

#[test]
fn a_box_without_an_image_makes_no_request() {
    let mut server = Server::new();
    let mock = server.mock("POST", "/v1/systemone").expect(0).create();

    let error = client(&server)
        .system_one(&json!({"photo": "none"}), &rectangle_questions())
        .unwrap_err();
    assert_eq!(error.kind(), ErrorKind::InvalidRequest);
    assert!(error.message().contains("needs an image"), "{error}");
    mock.assert();
}

#[test]
fn a_413_is_a_payload_too_large_error() {
    let mut server = Server::new();
    server
        .mock("POST", "/v1/systemone")
        .with_status(413)
        .with_body(
            r#"{"detail": {"error_type": "api_usage_error", "message": "Request too large."}}"#,
        )
        .create();

    let error = client(&server)
        .system_one(&ticket(), &questions())
        .unwrap_err();
    assert_eq!(error.kind(), ErrorKind::PayloadTooLarge);
    assert_eq!(error.message(), "Request too large.");
}

/// A state big enough to be worth compressing.
#[cfg(any(feature = "gzip", feature = "zstd"))]
fn long_ticket() -> serde_json::Value {
    json!({"ticket": "I was charged twice. Please fix this. ".repeat(100)})
}

#[cfg(any(feature = "gzip", feature = "zstd"))]
#[test]
fn compressed_requests_reach_the_server_intact() {
    use typesafe::Compression;

    let mut codecs: Vec<(Compression, &str)> = Vec::new();
    #[cfg(feature = "gzip")]
    codecs.push((Compression::Gzip, "gzip"));
    #[cfg(feature = "zstd")]
    codecs.push((Compression::Zstd, "zstd"));

    for (codec, encoding) in codecs {
        let mut server = Server::new();
        let mock = server
            .mock("POST", "/v1/systemone")
            .match_header("content-encoding", encoding)
            .match_header("content-type", "application/json")
            .match_request(move |request| {
                let raw = request.body().unwrap();
                let json = match encoding {
                    #[cfg(feature = "gzip")]
                    "gzip" => {
                        let mut out = Vec::new();
                        std::io::Read::read_to_end(
                            &mut flate2::read::GzDecoder::new(&raw[..]),
                            &mut out,
                        )
                        .unwrap();
                        out
                    }
                    #[cfg(feature = "zstd")]
                    "zstd" => zstd::decode_all(&raw[..]).unwrap(),
                    _ => unreachable!(),
                };
                let body: serde_json::Value = serde_json::from_slice(&json).unwrap();
                body["state"] == long_ticket() && body["questions"]["billing"]["type"] == "noul"
            })
            .with_body(answers().to_string())
            .create();

        let client = Client::with_config(config(&server.url()).compression(codec)).unwrap();
        let response = client.system_one(&long_ticket(), &questions()).unwrap();
        mock.assert();
        assert!(response.noul("billing").unwrap().yes());
    }
}

#[test]
fn a_missing_key_makes_no_request() {
    let mut server = Server::new();
    let mock = server.mock("POST", "/v1/systemone").expect(0).create();
    let client = Client::with_config(Config::new().base_url(server.url())).unwrap();

    assert!(!client.is_configured());
    let error = client.system_one(&ticket(), &questions()).unwrap_err();
    assert_eq!(error.kind(), ErrorKind::NoApiKey);
    mock.assert();
}

#[test]
fn retries_an_overloaded_server() {
    let mut server = Server::new();
    let first = server
        .mock("POST", "/v1/systemone")
        .match_header("x-typesafe-retry-count", Matcher::Missing)
        .with_status(529)
        .with_header("retry-after-ms", "1")
        .with_body(r#"{"error": "overloaded"}"#)
        .expect(1)
        .create();
    let retry = server
        .mock("POST", "/v1/systemone")
        .match_header("x-typesafe-retry-count", "1")
        .with_body(answers().to_string())
        .expect(1)
        .create();

    let response = client(&server).system_one(&ticket(), &questions()).unwrap();
    first.assert();
    retry.assert();
    assert_eq!(response.model, "jev-1.13.0");
}

#[test]
fn gives_up_after_max_retries() {
    let mut server = Server::new();
    let mock = server
        .mock("POST", "/v1/systemone")
        .with_status(503)
        .with_body(r#"{"detail": "down for maintenance"}"#)
        .expect(3)
        .create();

    let error = client(&server)
        .system_one(&ticket(), &questions())
        .unwrap_err();
    mock.assert();
    assert_eq!(error.kind(), ErrorKind::ServerError);
    assert_eq!(error.status(), Some(503));
    assert_eq!(error.message(), "down for maintenance");
}

#[test]
fn does_not_retry_a_bad_request() {
    let mut server = Server::new();
    let mock = server
        .mock("POST", "/v1/systemone")
        .with_status(422)
        .with_header("x-typesafe-request-id", "req_422")
        .with_body(
            json!({"detail": [
                {"loc": ["body", "questions", "tone", "criteria"], "msg": "field required"}
            ]})
            .to_string(),
        )
        .expect(1)
        .create();

    let error = client(&server)
        .system_one(&ticket(), &questions())
        .unwrap_err();
    mock.assert();
    assert_eq!(error.kind(), ErrorKind::UnprocessableEntity);
    assert_eq!(
        error.to_string(),
        "422 questions.tone.criteria: field required (request_id=req_422)"
    );
}

#[test]
fn a_wait_past_the_budget_is_not_attempted() {
    let mut server = Server::new();
    let mock = server
        .mock("POST", "/v1/systemone")
        .with_status(429)
        .with_header("retry-after", "60")
        .expect(1)
        .create();

    let error = client(&server)
        .system_one(&ticket(), &questions())
        .unwrap_err();
    mock.assert();
    assert_eq!(error.kind(), ErrorKind::RateLimited);
    assert_eq!(error.retry_after(), Some(Duration::from_secs(60)));
}

#[test]
fn per_call_options_override_the_client() {
    let mut server = Server::new();
    let mock = server
        .mock("POST", "/v1/systemone")
        .match_header("x-trace", "abc")
        .match_body(Matcher::PartialJson(
            json!({"model": "jev-1.13.0", "trace": true}),
        ))
        .with_status(500)
        .expect(1)
        .create();

    let options = CallOptions::new()
        .model("jev-1.13.0")
        .header("x-trace", "abc")
        .extra_body("trace", true)
        .retry(RetryPolicy::disabled());
    let error = client(&server)
        .system_one_with(&ticket(), &questions(), &options)
        .unwrap_err();
    mock.assert();
    assert_eq!(error.kind(), ErrorKind::ServerError);
    assert_eq!(error.message(), "(no body)");
}

#[test]
fn a_bad_answer_names_the_field() {
    let mut server = Server::new();
    let mut body = answers();
    body["answers"]["tone"]
        .as_object_mut()
        .unwrap()
        .remove("confidence");
    server
        .mock("POST", "/v1/systemone")
        .with_body(body.to_string())
        .create();

    let error = client(&server)
        .system_one(&ticket(), &questions())
        .unwrap_err();
    assert_eq!(error.kind(), ErrorKind::InvalidResponse);
    assert_eq!(error.status(), Some(200));
    assert!(
        error.message().contains("answers.tone.confidence"),
        "{error}"
    );
    assert!(error.body().is_some());
}

#[test]
fn a_silent_server_times_out() {
    // The kernel accepts the connection into the backlog, and nothing answers.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let config = config(&url)
        .timeout(Duration::from_millis(200))
        .retry(RetryPolicy::disabled());

    let error = Client::with_config(config)
        .unwrap()
        .system_one(&ticket(), &questions())
        .unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Timeout, "{error}");
    drop(listener);
}

#[test]
fn a_refused_connection_is_retried_then_reported() {
    let config = config(&closed_port_url());
    let error = Client::with_config(config)
        .unwrap()
        .system_one(&ticket(), &questions())
        .unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Connection, "{error}");
    assert!(std::error::Error::source(&error).is_some());
}

#[test]
fn lists_models() {
    let mut server = Server::new();
    let mock = server
        .mock("GET", "/v1/models")
        .match_header("authorization", "Bearer sk-test")
        .with_body(
            json!({"models": [
                {"name": "jev-latest", "description": "The newest Jev"},
                {"name": "jev-1.13.0", "release_date": "2026-06-01T00:00:00Z"}
            ]})
            .to_string(),
        )
        .create();

    let models = client(&server).models().unwrap();
    mock.assert();
    let names: Vec<&str> = models.iter().map(|m| m.name.as_str()).collect();
    assert_eq!(names, ["jev-latest", "jev-1.13.0"]);
    assert_eq!(
        models[1].release_date.as_deref(),
        Some("2026-06-01T00:00:00Z")
    );
}

#[test]
fn raw_questions_get_raw_answers() {
    let mut server = Server::new();
    server
        .mock("POST", "/v1/systemone")
        .match_body(Matcher::PartialJson(
            json!({"questions": {"future": {"type": "ranking", "items": ["a", "b"]}}}),
        ))
        .with_body(
            json!({
                "model": "jev-2.0.0",
                "answers": {"future": {"type": "ranking", "order": ["b", "a"]}},
                "usage": {"input_tokens": 10, "output_tokens": 2}
            })
            .to_string(),
        )
        .create();

    let questions = Questions::new().ask(
        "future",
        typesafe::Question::Raw(json!({"type": "ranking", "items": ["a", "b"]})),
    );
    let response = client(&server).system_one("some text", &questions).unwrap();
    assert_eq!(response.raw("future").unwrap()["order"], json!(["b", "a"]));
}

#[test]
fn invalid_questions_are_rejected_before_sending() {
    let mut server = Server::new();
    let mock = server.mock("POST", "/v1/systemone").expect(0).create();
    let questions = Questions::new().ask("q", Noul::new(""));

    let error = client(&server).system_one("text", &questions).unwrap_err();
    assert_eq!(error.kind(), ErrorKind::InvalidRequest);
    assert!(error.message().contains("question \"q\""), "{error}");
    mock.assert();
}
