//! Fixtures shared by the HTTP tests.

#![allow(dead_code)]

use std::time::Duration;

use serde_json::{Value, json};
use typesafe::{BoxQuestion, Choice, Config, Image, Noul, Questions, RetryPolicy, Score};

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

/// A 400x400 white image with one red rectangle at left 0.5, right 0.9, top 0.2,
/// bottom 0.5 (as fractions of the image).
pub fn red_rectangle() -> Image {
    Image::from_path(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/red-rectangle.png"
    ))
    .unwrap()
}

/// Where to find the rectangle, and whether it is there.
pub fn rectangle_questions() -> Questions {
    Questions::new()
        .ask("present", Noul::new("Is there a red rectangle in `photo`?"))
        .ask("rect", BoxQuestion::new("The red rectangle in `photo`"))
}

pub fn rectangle_answers() -> Value {
    json!({
        "model": "jev-1.14.0",
        "answers": {
            "present": {"type": "noul", "noul": 0.97},
            "rect": {"type": "box",
                     "box": {"top": 0.14, "bottom": 0.52, "left": 0.5, "right": 0.91},
                     "confidence": 0.95}
        },
        "usage": {"input_tokens": 755, "output_tokens": 36}
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
