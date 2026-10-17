//! A Rust client for [TypeSafe](https://docs.typesafe.ai/)'s System One API,
//! the API behind **Jev**. It is unofficial. Its behaviour follows the
//! official Python SDK: the same environment variables and defaults, the same
//! retry policy, and the same error kinds.
//!
//! System One models don't generate text. You send some state and a set of
//! typed questions, and get back one typed answer per question, with
//! probabilities your code acts on:
//!
//! | Question | Asks | Answer |
//! | --- | --- | --- |
//! | [`Noul`] | whether a condition holds | [`NoulAnswer`]: the probability of yes |
//! | [`Choice`] | which one of a defined set | [`ChoiceAnswer`]: option, probabilities, confidence |
//! | [`Score`] | how far along ordered levels | [`ScoreAnswer`]: weighted level, probabilities, confidence |
//! | [`BoxQuestion`] | where something is in an [`Image`] | [`BoxAnswer`]: the box's four edges, as fractions |
//!
//! ```no_run
//! use serde_json::json;
//! use typesafe::{Choice, Noul, Questions, Score};
//!
//! # #[cfg(feature = "blocking")]
//! # fn main() -> Result<(), typesafe::Error> {
//! let client = typesafe::blocking::Client::new(); // reads TYPESAFE_API_KEY
//!
//! let questions = Questions::new()
//!     .ask("billing", Noul::new("Is this ticket about billing?"))
//!     .ask("tone", Choice::new("What is the customer's tone?").options(["calm", "frustrated", "angry"]))
//!     .ask("urgency", Score::new("How urgent is this ticket?", ["Can wait", "This week", "Today"]));
//!
//! let response = client.system_one(
//!     &json!({"ticket": "I was charged twice. Please fix this ASAP."}),
//!     &questions,
//! )?;
//!
//! let tone = response.choice("tone").unwrap();
//! if response.noul("billing").unwrap().yes() && tone.confidence > 0.5 {
//!     println!("billing, {} ({:.2})", tone.choice, tone.confidence);
//! }
//! # Ok(())
//! # }
//! # #[cfg(not(feature = "blocking"))]
//! # fn main() {}
//! ```
//!
//! # Features
//!
//! - `async` (default): [`Client`], for a tokio runtime
//! - `blocking`: [`blocking::Client`], for synchronous code
//! - `rustls` (default) or `native-tls`: the TLS backend

mod answer;
mod config;
mod error;
mod image;
mod question;
mod request;
mod response;
mod retry;

#[cfg(feature = "blocking")]
pub mod blocking;
#[cfg(feature = "async")]
mod client;

pub use answer::{Answer, BoxAnswer, ChoiceAnswer, NoulAnswer, ScoreAnswer};
#[cfg(feature = "async")]
pub use client::Client;
pub use config::{
    CallOptions, Config, DEFAULT_BASE_URL, DEFAULT_MODEL, DEFAULT_TIMEOUT, ENV_API_KEY,
    ENV_BASE_URL, ENV_DEFAULT_MODEL,
};
pub use error::{Error, ErrorKind};
pub use image::Image;
pub use question::{BoxQuestion, Choice, Noul, Question, Questions, Score};
pub use response::{Model, Response, Usage};
pub use retry::RetryPolicy;
