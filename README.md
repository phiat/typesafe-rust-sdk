# TypeSafe for Rust

[![crates.io](https://img.shields.io/crates/v/typesafe-rust-sdk.svg)](https://crates.io/crates/typesafe-rust-sdk)
[![docs.rs](https://img.shields.io/docsrs/typesafe-rust-sdk)](https://docs.rs/typesafe-rust-sdk)
[![MSRV](https://img.shields.io/crates/msrv/typesafe-rust-sdk)](https://github.com/phiat/typesafe-rust-sdk/blob/main/Cargo.toml)
[![License: MIT](https://img.shields.io/crates/l/typesafe-rust-sdk)](LICENSE)

A Rust client for [TypeSafe](https://docs.typesafe.ai/)'s System One API, the API
behind **Jev**. It is unofficial and not published by TypeSafe. Its behaviour follows
the official Python SDK:

- the same environment variables and defaults
- the same retry policy
- the same error kinds

It has an async client, and a blocking one for synchronous code such as a game loop
or a CLI. It is the Rust sibling of
[typesafe-elixir-sdk](https://github.com/phiat/typesafe-elixir-sdk).

System One models don't generate text. You send some state and a set of typed
questions, and get back one typed answer per question, with probabilities your code
acts on.

```rust
use serde_json::json;
use typesafe::{Choice, Noul, Questions, Score};

let client = typesafe::blocking::Client::new(); // reads TYPESAFE_API_KEY

let questions = Questions::new()
    .ask("billing", Noul::new("Is this ticket about billing?"))
    .ask("tone", Choice::new("What is the customer's tone?").options(["calm", "frustrated", "angry"]))
    .ask("urgency", Score::new("How urgent is this ticket?", ["Can wait", "This week", "Today"]));

let response = client.system_one(
    &json!({"ticket": "I was charged twice. Please fix this ASAP."}),
    &questions,
)?;

response.noul("billing").unwrap().noul         // 0.99
response.choice("tone").unwrap().choice        // "frustrated"
response.choice("tone").unwrap().confidence    // 0.78
response.score("urgency").unwrap().score       // 1.99
response.model                                 // "jev-1.13.0"
response.usage                                 // Usage { input_tokens: 367, output_tokens: 75 }
```

## Installation

```toml
[dependencies]
typesafe-rust-sdk = "0.2"
```

That gives the async client. For the blocking one as well, use
`{ version = "0.2", features = ["blocking"] }`, or for the blocking one alone,
`{ version = "0.2", default-features = false, features = ["blocking", "rustls"] }`.

The library is named `typesafe`, so code says `use typesafe::...`.

| Feature | What it adds |
| --- | --- |
| `async` (default) | `typesafe::Client`, which needs a tokio runtime with the time driver (`#[tokio::main]` has it) |
| `blocking` | `typesafe::blocking::Client`, which must not be called from inside an async runtime |
| `rustls` (default), `native-tls` | the TLS backend; with neither, only `http://` base URLs work |
| `gzip` | `Compression::Gzip`, pure Rust ([compression](#compression)) |
| `zstd` | `Compression::Zstd`, faster and smaller than gzip, but needs a C compiler to build |

It uses `reqwest`, `serde_json`, `httpdate` and `fastrand`, plus `tokio` for the async
client, `flate2` for `gzip` and `zstd` for `zstd`.

## Questions and answers

| Question | Asks | Answer |
| --- | --- | --- |
| `Noul::new(instructions)` | whether a condition holds | `NoulAnswer`: `noul`, the probability of yes |
| `Choice::new(instructions)` | which one of a defined set | `ChoiceAnswer`: `choice`, `probabilities`, `confidence` |
| `Score::new(instructions, levels)` | how far along ordered levels | `ScoreAnswer`: `score`, `legend`, `probabilities`, `confidence` |
| `BoxQuestion::new(instructions)` | where something is in an [image](#images-and-box-questions) | `BoxAnswer`: `top`, `bottom`, `left`, `right`, `confidence` |

```rust
use typesafe::{Choice, Noul};

Choice::new("Which team should own this ticket?")
    .option("billing", "Charges, invoices, refunds")
    .option("technical", "Bugs, outages, integrations")
    .option("unclear", "The ticket does not say enough to tell");

Noul::new("If the user wants to change the lights, do they want them on?")
    .yes("Lights on or brighter")
    .no("Lights off or dimmer");
```

A few details keep answers easy to use:

- **Answers use the question's ids.** `response.choice("team")` returns the
  `ChoiceAnswer` asked under `"team"`. `response.answers` holds all of them.
- **Choice options keep their order.** They're sent in the order you added them,
  because the order is part of what the model reads.
- **State keeps its order.** `state` is a string or anything that serializes to a
  JSON object or array. A struct is sent with its fields in declaration order.
- **Score levels are indexes.** Keys in `legend` and `probabilities` are `usize`,
  not the strings on the wire.
- **Helpers:**
  - `ChoiceAnswer::ranked` gives options by probability, useful for a "did you mean"
    prompt.
  - `ChoiceAnswer::is` and `ChoiceAnswer::probability` look up one option.
  - `ScoreAnswer::normalized` scales the score to 0..=1.
  - `ScoreAnswer::level` gives the most probable level.
  - `NoulAnswer::yes` and `NoulAnswer::yes_above` compare the probability to a
    threshold.

Instructions and descriptions take anything that converts to `serde_json::Value`, so
`json!({...})` works when structure makes a question clearer. `Question::Raw(json)` is
sent as given, and its answer comes back as `Answer::Raw`. That covers question types
this client predates.

Malformed questions, such as a Choice with no options, are rejected with
`ErrorKind::InvalidRequest` before anything is sent. So are Choices with more than 255
options and Scores with more than 10 levels, which the API refuses.

## Images and Box questions

A `BoxQuestion` finds something in an image and answers with a box. Images work with
any question type: a Noul can ask whether a person is in a photo, a Choice what kind
of document a scan is.

An `Image` is a PNG or JPEG. It stands in a request by its *reference*, a string. Put
the `Image` where it belongs in the state or in a question, and attach it to the call:

```rust
use serde_json::json;
use typesafe::{BoxQuestion, CallOptions, Image, Noul, Questions};

let photo = Image::from_path("kitchen.jpg")?;

let questions = Questions::new()
    .ask("mug_present", Noul::new("Is there a coffee mug in `photo`?"))
    .ask("mug", BoxQuestion::new("The coffee mug in `photo`"));

let response = client.system_one_with(
    &json!({ "photo": photo }),
    &questions,
    &CallOptions::new().model("jev-1.14.0").image(&photo),
)?;

if response.noul("mug_present").unwrap().yes_above(0.5) {
    let mug = response.bounding_box("mug").unwrap();
    let (left, top, right, bottom) = mug.pixels(1024, 768); // for a 1024x768 image
}
```

- **Box needs `jev-1.14.0` or later**, or the `jev-preview` alias. `jev-latest` stays
  on `jev-1.13.0` until 1.14 is promoted, and a Box question sent to it is rejected
  with a plain `400`.
- **Edges are fractions.** `left` and `right` are fractions of the width, `top` and
  `bottom` of the height, from the top-left corner. `BoxAnswer::pixels` scales them
  to an image of known size.
- **A box always comes back.** There is no "not found" answer, so a target that isn't
  there still gets a box, with low confidence. Ask a Noul about presence in the same
  call, as above, and ignore the box when it is low. `BoxAnswer::confidence` is an
  `Option` because the API may leave it out.
- **The type is `BoxQuestion`**, and the accessor `bounding_box`, because `Box` would
  shadow `std::boxed::Box` and `box` is a reserved word.
- **Images work on `jev-1.13.0` too.** They are billed as input tokens, and each place
  a reference is used sends another copy of the image.
- **Reference the image by name or by value.** The state can be `json!({"photo":
  photo})` with questions saying `` `photo` ``, or you can write the reference into a
  string with `format!("... {photo} ...")`.
- **Mistakes are caught before sending.** An image nothing refers to, a reference
  whose image was never attached, and a Box question with no image all fail with
  `ErrorKind::InvalidRequest`. The API would otherwise answer with an opaque `400`, or
  for the last two, silently answer about text and not the picture.
- **Limits.** Each side must be 280 to 1568 pixels, a file at most 16 MiB, and the
  request at most 32 MiB. Images outside that are rejected by the API (`400`, or `413`
  as `ErrorKind::PayloadTooLarge`), not resized. Rotate pixels upright first: EXIF
  orientation is ignored. Convert GIF, WebP and HEIC to PNG or JPEG; `Image::from_bytes`
  refuses them.
- **Transport.** Images go as a `multipart/form-data` upload, which the API recommends
  over base64, and the `TypeSafe-Danger-Zone-Images: true` header it requires is added
  for you. Images are marked experimental by TypeSafe and may change.

## Code owns the decision

Ask independent questions together: they share one read of the state and run in
parallel. Then let ordinary code apply the policy.

```rust
let team = response.choice("team").unwrap();

let route = if response.noul("security").unwrap().yes_above(0.7) {
    "security"
} else if team.is("unclear") || team.confidence < 0.5 {
    "human"
} else {
    team.choice.as_str()
};
```

`confidence` measures how concentrated the distribution is. It is not the probability
that the answer is right. Tune thresholds on your own data, and pin a versioned model
such as `"jev-1.14.0"` once you have. The aliases move when a new release ships.

| Model | Notes |
| --- | --- |
| `jev-1.14.0` | the newest release (14 October 2026); the only one with Box questions |
| `jev-1.13.0` | the previous release |
| `jev-latest` | the newest *stable* release; the default. `jev-1.13.0` as of 16 October 2026 |
| `jev-preview` | the newest release of any kind. `jev-1.14.0` as of 16 October 2026 |

All models take 64k tokens per request and 32k per question. `client.models()` lists
what the account can send; versioned ids work whether or not they're listed. For a
state that doesn't fit, `CallOptions::long_context()` raises both limits to 262k tokens
at the same price per token, but slower and less accurate.

For many items, run calls concurrently. The client is cheap to clone and shares its
connection pool:

```rust
let handles: Vec<_> = tickets
    .into_iter()
    .map(|ticket| {
        let (client, questions) = (client.clone(), questions.clone());
        tokio::spawn(async move { client.system_one(&ticket, &questions).await })
    })
    .collect();
```

## Configuration

| `Config` setter | Environment variable | Default |
| --- | --- | --- |
| `api_key` | `TYPESAFE_API_KEY` | none |
| `base_url` | `TYPESAFE_BASE_URL` | `https://api.typesafe.ai` |
| `model` | `TYPESAFE_DEFAULT_MODEL` | `jev-latest` |
| `timeout` | | 10 s per response |
| `retry` | | see below |
| `compression` | | off; see [compression](#compression) |
| `header` | | extra request headers |

```rust
use std::time::Duration;
use typesafe::{Client, Config};

let client = Client::with_config(
    Config::from_env().model("jev-1.14.0").timeout(Duration::from_secs(5)),
)?;
```

`Client::new()` is `Config::from_env()` with no changes. `Config::new()` ignores the
environment. The API key is kept out of `Debug` output.

A missing key doesn't stop a client from being built, so an app can start without one.
Calls then fail with `ErrorKind::NoApiKey` without making a request.
`client.is_configured()` tells you which case you are in.

`system_one_with` takes `CallOptions` to override the model, timeout, retry policy
and headers for one call, to attach [images](#images-and-box-questions) with `image`,
and to raise the context limits with `long_context`. `CallOptions::extra_body` adds
top-level fields for API features this client predates.

## Compression

The API accepts request bodies compressed with gzip or zstd. It pays on large states,
such as a long document or a chat log, and saves upload time on a slow link. It is off
unless you ask, and each codec is a cargo feature, so a build only carries what it uses:

```toml
typesafe-rust-sdk = { version = "0.2", features = ["zstd"] }   # or "gzip"
```

```rust
use typesafe::{CallOptions, Compression, Config};

let config = Config::from_env().compression(Compression::Zstd);   // every call
let options = CallOptions::new().compression(Compression::Off);   // override one call
```

- **Bodies under 1 KiB are sent as they are**, as the official SDKs do. Larger ones
  get a `Content-Encoding` header.
- **Retries resend the same compressed bytes.** The body is compressed once per call.
- **Requests with images** are compressed whole. The images won't shrink, since PNG
  and JPEG are already compressed, so it only helps when the state is large text.
- **zstd** uses its default level (3), whose 2 MiB window is well inside the 8 MiB the
  API accepts. It is what the official Python SDK uses by default.

## Errors and retries

Every call returns `Result<_, typesafe::Error>`. An error has:

- `kind()`
- `status()`, the HTTP status
- `message()`, the server's message
- `request_id()`, from the `x-typesafe-request-id` header
- `retry_after()`
- `body()`

It displays as `422 questions.tone.criteria: field required (request_id=...)`.

| `ErrorKind` | When |
| --- | --- |
| `NoApiKey` | no key configured; no request made |
| `InvalidRequest` | a malformed call, such as a Choice with no options; no request made |
| `BadRequest`, `Authentication`, `PermissionDenied`, `NotFound` | 400, 401, 403, 404 |
| `PayloadTooLarge` | 413; the request is over 32 MiB, as it can be with images |
| `UnprocessableEntity` | 422; the message lists each field as `path: problem` |
| `RateLimited`, `Overloaded`, `ServerError`, `HttpError` | 429, 529, other 5xx, anything else |
| `Timeout`, `Connection` | no response |
| `InvalidResponse` | a 2xx missing a required field; the message names it |

Retries follow the official SDKs:

- **What is retried:** up to 2 retries on 408, 429 and every 5xx, and on timeouts and
  failed connections.
- **Backoff:** from 500 ms, doubling to 5 s, with 25% jitter.
- **Server waits:** `retry-after-ms` and `retry-after` are honoured.
- **Budget:** 30 s per call, including waits. A retry whose wait would pass the budget
  is not attempted.
- **Retry count:** each retry sends `x-typesafe-retry-count`.

```rust
use typesafe::RetryPolicy;

let patient = RetryPolicy { max_retries: 5, ..RetryPolicy::default() };
let never = RetryPolicy::disabled();
```

## Examples

```sh
TYPESAFE_API_KEY=... cargo run --example triage --features blocking   # routing tickets
TYPESAFE_API_KEY=... cargo run --example factcheck                    # concurrent calls
TYPESAFE_API_KEY=... cargo run --example locate --features blocking -- photo.jpg "the dog"   # Box
```

## Developing

```sh
cargo test --all-features                                        # unit and mock-server tests, no network
TYPESAFE_API_KEY=... cargo test --all-features -- --ignored      # the live API
```

TypeSafe docs: [index](https://docs.typesafe.ai/llms.txt) ·
[primitives](https://docs.typesafe.ai/primitives) ·
[confidence](https://docs.typesafe.ai/confidence) ·
[HTTP API](https://docs.typesafe.ai/api)

## License

MIT. See [LICENSE](LICENSE).
