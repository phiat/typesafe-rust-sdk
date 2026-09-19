//! Support triage with the blocking client: three questions per ticket in one
//! request, and ordinary code owns the routing.
//!
//! ```sh
//! TYPESAFE_API_KEY=... cargo run --example triage --features blocking
//! ```

use serde_json::json;
use typesafe::blocking::Client;
use typesafe::{Choice, Noul, Questions, Score};

const TICKETS: &[&str] = &[
    "I was charged twice for my order. Please refund one.",
    "The export button has thrown a 500 error since this morning.",
    "Someone logged into my account from another country and changed my email.",
    "hey, quick one: do you have a dark mode?",
];

fn main() -> Result<(), typesafe::Error> {
    let client = Client::new();
    if !client.is_configured() {
        eprintln!("set TYPESAFE_API_KEY to run this example");
        std::process::exit(1);
    }

    let questions = Questions::new()
        .ask(
            "team",
            Choice::new("Which team should own this ticket?")
                .option("billing", "Charges, invoices, refunds")
                .option("technical", "Bugs, outages, errors, integrations")
                .option("account", "Logins, passwords, profile settings")
                .option("unclear", "The ticket does not say enough to tell"),
        )
        .ask(
            "security",
            Noul::new("Does the ticket describe a possible account compromise or data exposure?"),
        )
        .ask(
            "urgency",
            Score::new(
                "How urgently does this ticket need a response?",
                [
                    "Can wait: a question or feedback, nothing is blocked",
                    "Soon: a problem with a workaround",
                    "Today: something important is broken for the customer",
                ],
            ),
        );

    for ticket in TICKETS {
        let response = client.system_one(&json!({ "ticket": ticket }), &questions)?;
        let team = response.choice("team").unwrap();
        let security = response.noul("security").unwrap();
        let urgency = response.score("urgency").unwrap();

        // Code owns the decision: a security flag overrides the team, and a
        // low-confidence team goes to a person.
        let route = if security.yes_above(0.7) {
            "security".to_string()
        } else if team.is("unclear") || team.confidence < 0.5 {
            "human review".to_string()
        } else {
            team.choice.clone()
        };
        println!(
            "{route:>13}  urgency {:.1}/2  ({} {:.2})  {ticket}",
            urgency.score, team.choice, team.confidence
        );
        println!(
            "{:>13}  {} · {} input tokens",
            "", response.model, response.usage.input_tokens
        );
    }
    Ok(())
}
