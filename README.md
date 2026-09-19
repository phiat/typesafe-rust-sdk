# TypeSafe for Rust

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
typesafe-rust-sdk = "0.1"
```

That gives the async client. For the blocking one as well, use
`{ version = "0.1", features = ["blocking"] }`, or for the blocking one alone,
`{ version = "0.1", default-features = false, features = ["blocking", "rustls"] }`.

The library is named `typesafe`, so code says `use typesafe::...`.

| Feature | What it adds |
| --- | --- |
| `async` (default) | `typesafe::Client`, which needs a tokio runtime with the time driver (`#[tokio::main]` has it) |
| `blocking` | `typesafe::blocking::Client`, which must not be called from inside an async runtime |
| `rustls` (default), `native-tls` | the TLS backend; with neither, only `http://` base URLs work |

It uses `reqwest`, `serde_json`, `httpdate` and `fastrand`, plus `tokio` for the async
client.

## Questions and answers

| Question | Asks | Answer |
| --- | --- | --- |
| `Noul::new(instructions)` | whether a condition holds | `NoulAnswer`: `noul`, the probability of yes |
| `Choice::new(instructions)` | which one of a defined set | `ChoiceAnswer`: `choice`, `probabilities`, `confidence` |
| `Score::new(instructions, levels)` | how far along ordered levels | `ScoreAnswer`: `score`, `legend`, `probabilities`, `confidence` |

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
`ErrorKind::InvalidRequest` before anything is sent.

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
such as `"jev-1.13.0"` once you have. `jev-latest` moves when a new release ships.

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
| `header` | | extra request headers |

```rust
use std::time::Duration;
use typesafe::{Client, Config};

let client = Client::with_config(
    Config::from_env().model("jev-1.13.0").timeout(Duration::from_secs(5)),
)?;
```

`Client::new()` is `Config::from_env()` with no changes. `Config::new()` ignores the
environment. The API key is kept out of `Debug` output.

A missing key doesn't stop a client from being built, so an app can start without one.
Calls then fail with `ErrorKind::NoApiKey` without making a request.
`client.is_configured()` tells you which case you are in.

`system_one_with` takes `CallOptions` to override the model, timeout, retry policy
and headers for one call. `CallOptions::extra_body` adds top-level fields for API
features this client predates.

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
