//! Responses from the API.

use std::collections::BTreeMap;

use serde_json::Value;

use crate::answer::{self, Answer, BoxAnswer, ChoiceAnswer, NoulAnswer, ScoreAnswer};
use crate::question::Questions;

/// Tokens used by a call. Only input tokens are billed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

/// The result of a System One call.
///
/// - `answers`: one answer per question, under the id the question used
/// - `model`: the versioned model that answered, such as `jev-1.14.0`, even
///   when the call named an alias like `jev-latest`. Store it next to anything
///   you keep, so a threshold can be traced to the model it was tuned on.
/// - `usage`: tokens used
/// - `request_id`: the `x-typesafe-request-id` header, for support requests
/// - `body`: the decoded JSON body, for fields this client doesn't know yet
#[derive(Clone, Debug, PartialEq)]
pub struct Response {
    pub model: String,
    pub answers: BTreeMap<String, Answer>,
    pub usage: Usage,
    pub request_id: Option<String>,
    pub body: Value,
}

impl Response {
    pub fn answer(&self, id: &str) -> Option<&Answer> {
        self.answers.get(id)
    }

    /// The answer to the Noul asked under `id`.
    pub fn noul(&self, id: &str) -> Option<&NoulAnswer> {
        self.answer(id).and_then(Answer::as_noul)
    }

    /// The answer to the Choice asked under `id`.
    pub fn choice(&self, id: &str) -> Option<&ChoiceAnswer> {
        self.answer(id).and_then(Answer::as_choice)
    }

    /// The answer to the Score asked under `id`.
    pub fn score(&self, id: &str) -> Option<&ScoreAnswer> {
        self.answer(id).and_then(Answer::as_score)
    }

    /// The answer to the [`BoxQuestion`](crate::BoxQuestion) asked under `id`.
    /// Not named `box`, which Rust reserves.
    pub fn bounding_box(&self, id: &str) -> Option<&BoxAnswer> {
        self.answer(id).and_then(Answer::as_box)
    }

    /// The answer to the raw question asked under `id`.
    pub fn raw(&self, id: &str) -> Option<&Value> {
        self.answer(id).and_then(Answer::as_raw)
    }
}

/// A model name the account can send, from `models()`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Model {
    pub name: String,
    pub description: Option<String>,
    /// As the server sent it, usually an ISO 8601 timestamp.
    pub release_date: Option<String>,
}

pub(crate) fn decode(
    body: &Value,
    questions: &Questions,
    request_id: Option<String>,
) -> Result<Response, String> {
    let model = body
        .get("model")
        .and_then(Value::as_str)
        .ok_or_else(|| "model".to_string())?
        .to_string();
    let usage = body.get("usage").ok_or_else(|| "usage".to_string())?;
    let tokens = |field: &str| {
        usage
            .get(field)
            .and_then(Value::as_u64)
            .ok_or_else(|| format!("usage.{field}"))
    };
    let usage = Usage {
        input_tokens: tokens("input_tokens")?,
        output_tokens: tokens("output_tokens")?,
    };
    let answers = answer::decode_all(questions, body.get("answers"))?;
    Ok(Response {
        model,
        answers,
        usage,
        request_id,
        body: body.clone(),
    })
}

pub(crate) fn decode_models(body: &Value) -> Result<Vec<Model>, String> {
    let models = body
        .get("models")
        .and_then(Value::as_array)
        .ok_or_else(|| "models".to_string())?;
    models
        .iter()
        .enumerate()
        .map(|(i, raw)| {
            let name = raw
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| format!("models[{i}].name"))?;
            let text = |field: &str| raw.get(field).and_then(Value::as_str).map(str::to_string);
            Ok(Model {
                name: name.to_string(),
                description: text("description"),
                release_date: text("release_date"),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Noul;
    use serde_json::json;

    #[test]
    fn decodes_a_response() {
        let questions = Questions::new().ask("billing", Noul::new("Billing?"));
        let body = json!({
            "model": "jev-1.13.0",
            "answers": {"billing": {"type": "noul", "noul": 0.97}},
            "usage": {"input_tokens": 100, "output_tokens": 10},
            "later": "a field this client doesn't know"
        });
        let response = decode(&body, &questions, Some("req_1".into())).unwrap();
        assert_eq!(response.model, "jev-1.13.0");
        assert_eq!(response.noul("billing").unwrap().noul, 0.97);
        assert!(response.choice("billing").is_none());
        assert_eq!(response.usage.input_tokens, 100);
        assert_eq!(response.request_id.as_deref(), Some("req_1"));
        assert_eq!(response.body["later"], "a field this client doesn't know");
    }

    #[test]
    fn names_missing_top_level_fields() {
        let questions = Questions::new().ask("q", Noul::new("Is it?"));
        let answers = json!({"q": {"type": "noul", "noul": 0.5}});
        assert_eq!(
            decode(
                &json!({"answers": answers, "usage": {"input_tokens": 1, "output_tokens": 1}}),
                &questions,
                None
            )
            .unwrap_err(),
            "model"
        );
        assert_eq!(
            decode(
                &json!({"model": "m", "answers": answers, "usage": {"input_tokens": 1}}),
                &questions,
                None
            )
            .unwrap_err(),
            "usage.output_tokens"
        );
    }

    #[test]
    fn decodes_models() {
        let models = decode_models(&json!({"models": [
            {"name": "jev-latest", "description": "Alias", "release_date": "2026-01-01T00:00:00Z"},
            {"name": "jev-1.13.0"}
        ]}))
        .unwrap();
        assert_eq!(models[1].name, "jev-1.13.0");
        assert_eq!(models[1].description, None);
        assert_eq!(
            decode_models(&json!({"models": [{}]})).unwrap_err(),
            "models[0].name"
        );
    }
}
