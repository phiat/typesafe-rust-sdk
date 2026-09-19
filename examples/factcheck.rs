//! Fact-checking with the async client: one request per claim, all in flight
//! at once.
//!
//! ```sh
//! TYPESAFE_API_KEY=... cargo run --example factcheck
//! ```

use serde_json::json;
use typesafe::{Choice, Client, Questions};

const SOURCE: &str = "The Ring Warden guards the age rings at the top of the Underbark. \
    It is too heavy to fall out of the tree, so a cut branch only makes it claw onto another.";

const CLAIMS: &[&str] = &[
    "The Ring Warden can't be dropped out of the tree.",
    "The Ring Warden lives in the Sapwood.",
    "The Ring Warden drops a card that unlocks a meta slot.",
];

#[tokio::main]
async fn main() -> Result<(), typesafe::Error> {
    let client = Client::new();
    if !client.is_configured() {
        eprintln!("set TYPESAFE_API_KEY to run this example");
        std::process::exit(1);
    }

    let questions = Questions::new().ask(
        "verdict",
        Choice::new("Does the source passage support the claim?")
            .option("supported", "The source says this")
            .option("contradicted", "The source says otherwise")
            .option("not_mentioned", "The source does not say either way"),
    );

    let calls = CLAIMS.iter().map(|claim| {
        let (client, questions) = (client.clone(), questions.clone());
        async move {
            let state = json!({ "source": SOURCE, "claim": claim });
            (claim, client.system_one(&state, &questions).await)
        }
    });
    let mut handles = Vec::new();
    for call in calls {
        handles.push(tokio::spawn(call));
    }
    for handle in handles {
        let (claim, response) = handle.await.expect("task panicked");
        let verdict = response?;
        let verdict = verdict.choice("verdict").unwrap();
        println!(
            "{:>13} ({:.2})  {claim}",
            verdict.choice, verdict.confidence
        );
    }
    Ok(())
}
