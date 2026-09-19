//! Fixtures shared by the HTTP tests.

#![allow(dead_code)]

use std::time::Duration;

use serde_json::{Value, json};
use typesafe::{Choice, Config, Noul, Questions, RetryPolicy, Score};

pub fn questions() -> Questions {
    Questions::new()
        .ask("billing", Noul::new("Is this ticket about billing?"))
        .ask(
            "tone",
            Choice::new("What is the customer's tone?").options(["calm", "frustrated", "angry"]),
        )
        .ask(
            "urgency",
            Score::new(
                "How urgent is this ticket?",
                ["Can wait", "This week", "Today"],
            ),
        )
}

pub fn answers() -> Value {
    json!({
        "model": "jev-1.13.0",
        "answers": {
            "billing": {"type": "noul", "noul": 0.99},
            "tone": {"type": "choice", "choice": "frustrated",
                     "probabilities": {"calm": 0.0, "frustrated": 0.86, "angry": 0.14},
                     "confidence": 0.78},
            "urgency": {"type": "score", "score": 1.99,
                        "legend": {"0": "Can wait", "1": "This week", "2": "Today"},
                        "probabilities": {"0": 0.0, "1": 0.01, "2": 0.99},
                        "confidence": 0.99}
        },
        "usage": {"input_tokens": 367, "output_tokens": 75}
    })
}

/// Retries without real waits, so the tests stay fast.
pub fn quick_retries() -> RetryPolicy {
    RetryPolicy {
        backoff_initial: Duration::from_millis(1),
        backoff_max: Duration::from_millis(5),
        ..RetryPolicy::default()
    }
}

pub fn config(url: &str) -> Config {
    Config::new()
        .api_key("sk-test")
        .base_url(url)
        .retry(quick_retries())
}

/// The state every test sends.
pub fn ticket() -> Value {
    json!({"ticket": "I was charged twice. Please fix this ASAP."})
}

/// A port where nothing listens.
pub fn closed_port_url() -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    format!("http://127.0.0.1:{port}")
}
