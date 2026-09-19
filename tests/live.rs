//! Against the live API. Ignored by default, since they need a key and cost a
//! fraction of a cent:
//!
//! ```sh
//! TYPESAFE_API_KEY=... cargo test --all-features -- --ignored
//! ```

#![cfg(feature = "blocking")]

mod common;

use typesafe::blocking::Client;
use typesafe::{Config, ErrorKind, Noul, Questions};

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
fn a_bad_key_is_an_authentication_error() {
    let client = Client::with_config(Config::from_env().api_key("not-a-key")).unwrap();
    let questions = Questions::new().ask("q", Noul::new("Is it?"));
    let error = client.system_one("text", &questions).unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Authentication, "{error}");
    assert_eq!(error.status(), Some(401));
}
