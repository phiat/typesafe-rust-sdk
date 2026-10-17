//! Against the live API. Ignored by default, since they need a key and cost a
//! fraction of a cent:
//!
//! ```sh
//! TYPESAFE_API_KEY=... cargo test --all-features -- --ignored
//! ```

#![cfg(feature = "blocking")]

mod common;

use typesafe::blocking::Client;
use typesafe::{CallOptions, Config, ErrorKind, Image, Noul, Questions};

fn client() -> Client {
    let client = Client::new();
    assert!(
        client.is_configured(),
        "set TYPESAFE_API_KEY to run the live tests"
    );
    client
}

#[test]
#[ignore = "calls the live API"]
fn lists_models() {
    let models = client().models().unwrap();
    assert!(models.iter().any(|m| m.name == "jev-latest"), "{models:?}");
}

#[test]
#[ignore = "calls the live API"]
fn answers_all_three_types_in_one_call() {
    let state =
        serde_json::json!({"ticket": "I was charged twice for my order. Please refund one."});
    let response = client().system_one(&state, &common::questions()).unwrap();

    assert!(response.model.starts_with("jev-"), "{}", response.model);
    assert!(response.request_id.is_some());
    assert!(response.usage.input_tokens > 0);

    assert!(response.noul("billing").unwrap().yes());
    let tone = response.choice("tone").unwrap();
    assert!(["calm", "frustrated", "angry"].contains(&tone.choice.as_str()));
    assert!((tone.probabilities.values().sum::<f64>() - 1.0).abs() < 0.01);
    let urgency = response.score("urgency").unwrap();
    assert!((0.0..=2.0).contains(&urgency.score));
    assert_eq!(
        urgency.legend.keys().copied().collect::<Vec<_>>(),
        [0, 1, 2]
    );
}

#[test]
#[ignore = "calls the live API"]
fn locates_a_rectangle_in_a_png() {
    let photo = common::red_rectangle();
    let response = client()
        .system_one_with(
            &serde_json::json!({"photo": photo}),
            &common::rectangle_questions(),
            // A Box question needs jev-1.14.0 (or jev-preview); jev-latest was 1.13.
            &CallOptions::new().model("jev-1.14.0").image(&photo),
        )
        .unwrap();

    assert_eq!(response.model, "jev-1.14.0");
    assert!(response.noul("present").unwrap().yes());
    let rect = response.bounding_box("rect").unwrap();
    assert!(rect.left < rect.right && rect.top < rect.bottom, "{rect:?}");
    // The rectangle is at left 0.5, right 0.9, top 0.2, bottom 0.5.
    for (got, want) in [
        (rect.left, 0.5),
        (rect.right, 0.9),
        (rect.top, 0.2),
        (rect.bottom, 0.5),
    ] {
        assert!(
            (got - want).abs() < 0.15,
            "{got} is not near {want}: {rect:?}"
        );
    }
    assert!(rect.confidence.unwrap() > 0.5);
}

#[test]
#[ignore = "calls the live API"]
fn reads_a_jpeg_on_the_default_model() {
    let photo = Image::from_path(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/red-rectangle.jpg"
    ))
    .unwrap();
    let questions = Questions::new().ask("red", Noul::new("Does `photo` contain a red shape?"));
    let response = client()
        .system_one_with(
            &serde_json::json!({"photo": photo}),
            &questions,
            &CallOptions::new().image(&photo),
        )
        .unwrap();
    assert!(response.noul("red").unwrap().yes());
}

#[test]
#[ignore = "calls the live API"]
fn accepts_the_long_context_header() {
    let questions = Questions::new().ask("hello", Noul::new("Is this a greeting?"));
    let response = client()
        .system_one_with(
            "Hello there!",
            &questions,
            &CallOptions::new().long_context(),
        )
        .unwrap();
    assert!(response.noul("hello").unwrap().yes());
}

/// The API takes each codec the crate was built with, for plain and multipart bodies.
#[cfg(any(feature = "gzip", feature = "zstd"))]
#[test]
#[ignore = "calls the live API"]
fn accepts_compressed_requests() {
    use typesafe::Compression;

    let codecs: &[Compression] = &[
        #[cfg(feature = "gzip")]
        Compression::Gzip,
        #[cfg(feature = "zstd")]
        Compression::Zstd,
    ];
    let log = "The customer was charged twice and wants one charge refunded. ".repeat(80);
    let photo = common::red_rectangle();
    for &codec in codecs {
        // A long text state.
        let questions = Questions::new().ask("billing", Noul::new("Is this about billing?"));
        let response = client()
            .system_one_with(&log, &questions, &CallOptions::new().compression(codec))
            .unwrap();
        assert!(response.noul("billing").unwrap().yes(), "{codec:?}");

        // A multipart body with an image in it.
        let response = client()
            .system_one_with(
                &serde_json::json!({"photo": photo, "log": log}),
                &common::rectangle_questions(),
                &CallOptions::new()
                    .model("jev-1.14.0")
                    .image(&photo)
                    .compression(codec),
            )
            .unwrap();
        assert!(response.noul("present").unwrap().yes(), "{codec:?}");
        assert!(
            response.bounding_box("rect").unwrap().left < 0.7,
            "{codec:?}"
        );
    }
}

#[test]
#[ignore = "calls the live API"]
fn a_bad_key_is_an_authentication_error() {
    let client = Client::with_config(Config::from_env().api_key("not-a-key")).unwrap();
    let questions = Questions::new().ask("q", Noul::new("Is it?"));
    let error = client.system_one("text", &questions).unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Authentication, "{error}");
    assert_eq!(error.status(), Some(401));
}
