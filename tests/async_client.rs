//! The async client against a local mock server.

#![cfg(feature = "async")]

mod common;

use mockito::{Matcher, Server};
use serde_json::json;
use typesafe::{CallOptions, Client, Config, ErrorKind};

use common::*;

#[tokio::test]
async fn answers_all_three_types() {
    let mut server = Server::new_async().await;
    let mock = server
        .mock("POST", "/v1/systemone")
        .match_header("authorization", "Bearer sk-test")
        .match_body(Matcher::PartialJson(json!({"state": ticket()})))
        .with_header("x-typesafe-request-id", "req_async")
        .with_body(answers().to_string())
        .create_async()
        .await;

    let client = Client::with_config(config(&server.url())).unwrap();
    let response = client.system_one(&ticket(), &questions()).await.unwrap();
    mock.assert_async().await;

    assert_eq!(response.request_id.as_deref(), Some("req_async"));
    assert_eq!(response.noul("billing").unwrap().noul, 0.99);
    assert_eq!(response.choice("tone").unwrap().choice, "frustrated");
    assert!((response.score("urgency").unwrap().normalized() - 0.995).abs() < 1e-9);
}

#[tokio::test]
async fn locates_something_in_an_image() {
    let mut server = Server::new_async().await;
    let mock = server
        .mock("POST", "/v1/systemone")
        .match_header("typesafe-danger-zone-images", "true")
        .match_header(
            "content-type",
            Matcher::Regex("^multipart/form-data; ".into()),
        )
        .with_body(rectangle_answers().to_string())
        .create_async()
        .await;

    let photo = red_rectangle();
    let client = Client::with_config(config(&server.url())).unwrap();
    let response = client
        .system_one_with(
            &json!({"photo": photo}),
            &rectangle_questions(),
            &CallOptions::new().image(&photo),
        )
        .await
        .unwrap();
    mock.assert_async().await;

    let (left, top, right, bottom) = response.bounding_box("rect").unwrap().pixels(400, 400);
    for (got, want) in [(left, 200.0), (top, 56.0), (right, 364.0), (bottom, 208.0)] {
        assert!((got - want).abs() < 1e-9, "{got} != {want}");
    }
}

#[tokio::test]
async fn retries_a_rate_limit() {
    let mut server = Server::new_async().await;
    let first = server
        .mock("POST", "/v1/systemone")
        .match_header("x-typesafe-retry-count", Matcher::Missing)
        .with_status(429)
        .with_header("retry-after-ms", "1")
        .expect(1)
        .create_async()
        .await;
    let second = server
        .mock("POST", "/v1/systemone")
        .match_header("x-typesafe-retry-count", "1")
        .with_status(529)
        .expect(1)
        .create_async()
        .await;
    let third = server
        .mock("POST", "/v1/systemone")
        .match_header("x-typesafe-retry-count", "2")
        .with_body(answers().to_string())
        .expect(1)
        .create_async()
        .await;

    let client = Client::with_config(config(&server.url())).unwrap();
    let response = client.system_one(&ticket(), &questions()).await;
    first.assert_async().await;
    second.assert_async().await;
    third.assert_async().await;
    assert!(response.is_ok());
}

#[tokio::test]
async fn a_missing_key_makes_no_request() {
    let client = Client::with_config(Config::new().base_url(closed_port_url())).unwrap();
    let error = client.models().await.unwrap_err();
    assert_eq!(error.kind(), ErrorKind::NoApiKey);
}

#[tokio::test]
async fn concurrent_calls_share_a_client() {
    let mut server = Server::new_async().await;
    let mock = server
        .mock("POST", "/v1/systemone")
        .with_body(answers().to_string())
        .expect(8)
        .create_async()
        .await;

    let client = Client::with_config(config(&server.url())).unwrap();
    let handles: Vec<_> = (0..8)
        .map(|_| {
            let client = client.clone();
            tokio::spawn(async move { client.system_one(&ticket(), &questions()).await })
        })
        .collect();
    for handle in handles {
        assert!(handle.await.unwrap().is_ok());
    }
    mock.assert_async().await;
}
