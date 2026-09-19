//! Answers: one per question, typed by the question that asked it.

use std::collections::BTreeMap;

use serde_json::{Map, Value};

use crate::question::{Question, Questions};

/// The answer to a [`Noul`](crate::Noul): `noul` is the probability of yes,
/// from 0 to 1.
///
/// A value near 0.5 means yes and no are about equally likely, not "somewhat
/// yes". Noul answers carry no separate confidence.
#[derive(Clone, Debug, PartialEq)]
pub struct NoulAnswer {
    pub noul: f64,
}

impl NoulAnswer {
    /// Whether the probability of yes is above 0.5.
    pub fn yes(&self) -> bool {
        self.noul > 0.5
    }

    /// Whether the probability of yes is above `threshold`.
    pub fn yes_above(&self, threshold: f64) -> bool {
        self.noul > threshold
    }
}

/// The answer to a [`Choice`](crate::Choice).
///
/// - `choice`: the most probable option
/// - `probabilities`: every option mapped to its probability; they sum to 1
/// - `confidence`: how concentrated the distribution is, from 0 to 1. It is
///   not the probability that the choice is right.
#[derive(Clone, Debug, PartialEq)]
pub struct ChoiceAnswer {
    pub choice: String,
    pub probabilities: BTreeMap<String, f64>,
    pub confidence: f64,
}

impl ChoiceAnswer {
    /// Whether `option` is the chosen one.
    pub fn is(&self, option: &str) -> bool {
        self.choice == option
    }

    /// The probability of `option`, or 0 if the answer doesn't list it.
    pub fn probability(&self, option: &str) -> f64 {
        self.probabilities.get(option).copied().unwrap_or(0.0)
    }

    /// The options with their probabilities, most probable first. Useful for a
    /// "did you mean" prompt.
    pub fn ranked(&self) -> Vec<(&str, f64)> {
        let mut ranked: Vec<(&str, f64)> = self
            .probabilities
            .iter()
            .map(|(option, &p)| (option.as_str(), p))
            .collect();
        ranked.sort_by(|a, b| b.1.total_cmp(&a.1));
        ranked
    }
}

/// The answer to a [`Score`](crate::Score).
///
/// - `score`: the probability-weighted level index. With four levels it lies
///   between 0 and 3 and can fall between levels.
/// - `legend`: each level index mapped to the description the question gave it
/// - `probabilities`: each level index mapped to its probability
/// - `confidence`: how concentrated the distribution is, from 0 to 1
///
/// Compare `score` against thresholds. Don't use it to recover an exact
/// quantity between two levels.
#[derive(Clone, Debug, PartialEq)]
pub struct ScoreAnswer {
    pub score: f64,
    pub legend: BTreeMap<usize, Value>,
    pub probabilities: BTreeMap<usize, f64>,
    pub confidence: f64,
}

impl ScoreAnswer {
    /// The score scaled to 0..=1, where 0 is the first level and 1 the last.
    pub fn normalized(&self) -> f64 {
        match self.legend.len() {
            0 | 1 => 0.0,
            n => self.score / (n - 1) as f64,
        }
    }

    /// The index of the most probable level.
    pub fn level(&self) -> usize {
        self.probabilities
            .iter()
            .max_by(|a, b| a.1.total_cmp(b.1).then(b.0.cmp(a.0)))
            .map_or(0, |(&level, _)| level)
    }
}

/// Any one answer. Its variant matches the question that asked it.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum Answer {
    Noul(NoulAnswer),
    Choice(ChoiceAnswer),
    Score(ScoreAnswer),
    /// The answer to a [`Question::Raw`], as the server sent it.
    Raw(Value),
}

impl Answer {
    pub fn as_noul(&self) -> Option<&NoulAnswer> {
        match self {
            Answer::Noul(a) => Some(a),
            _ => None,
        }
    }

    pub fn as_choice(&self) -> Option<&ChoiceAnswer> {
        match self {
            Answer::Choice(a) => Some(a),
            _ => None,
        }
    }

    pub fn as_score(&self) -> Option<&ScoreAnswer> {
        match self {
            Answer::Score(a) => Some(a),
            _ => None,
        }
    }

    pub fn as_raw(&self) -> Option<&Value> {
        match self {
            Answer::Raw(a) => Some(a),
            _ => None,
        }
    }
}

/// Decodes every question's answer. On failure, returns the path of the first
/// field that is missing or malformed, such as `answers.tone.confidence`, so a
/// bad response is reported rather than half read.
pub(crate) fn decode_all(
    questions: &Questions,
    answers: Option<&Value>,
) -> Result<BTreeMap<String, Answer>, String> {
    let answers = answers
        .and_then(Value::as_object)
        .ok_or_else(|| "answers".to_string())?;
    questions
        .iter()
        .map(|(id, question)| {
            let raw = answers.get(id).ok_or_else(|| format!("answers.{id}"))?;
            let answer = decode(question, raw).map_err(|field| format!("answers.{id}.{field}"))?;
            Ok((id.to_string(), answer))
        })
        .collect()
}

fn decode(question: &Question, raw: &Value) -> Result<Answer, String> {
    if let Question::Raw(_) = question {
        return Ok(Answer::Raw(raw.clone()));
    }
    let raw = raw.as_object().ok_or_else(|| "type".to_string())?;
    match question {
        Question::Noul(_) => {
            expect_type(raw, "noul")?;
            Ok(Answer::Noul(NoulAnswer {
                noul: number(raw, "noul")?,
            }))
        }
        Question::Choice(_) => {
            expect_type(raw, "choice")?;
            let choice = raw
                .get("choice")
                .and_then(Value::as_str)
                .ok_or_else(|| "choice".to_string())?;
            let probabilities = probabilities(raw)?
                .into_iter()
                .map(|(option, p)| (option.clone(), p))
                .collect();
            Ok(Answer::Choice(ChoiceAnswer {
                choice: choice.to_string(),
                probabilities,
                confidence: number(raw, "confidence")?,
            }))
        }
        Question::Score(_) => {
            expect_type(raw, "score")?;
            let score = number(raw, "score")?;
            let legend = raw
                .get("legend")
                .and_then(Value::as_object)
                .ok_or_else(|| "legend".to_string())?;
            let legend = index_keys(legend, "legend", |v| Some(v.clone()))?;
            let probabilities = raw
                .get("probabilities")
                .and_then(Value::as_object)
                .ok_or_else(|| "probabilities".to_string())?;
            let probabilities = index_keys(probabilities, "probabilities", Value::as_f64)?;
            Ok(Answer::Score(ScoreAnswer {
                score,
                legend,
                probabilities,
                confidence: number(raw, "confidence")?,
            }))
        }
        Question::Raw(_) => unreachable!("handled above"),
    }
}

fn expect_type(raw: &Map<String, Value>, expected: &str) -> Result<(), String> {
    match raw.get("type").and_then(Value::as_str) {
        Some(t) if t == expected => Ok(()),
        _ => Err("type".into()),
    }
}

fn number(raw: &Map<String, Value>, field: &str) -> Result<f64, String> {
    raw.get(field)
        .and_then(Value::as_f64)
        .ok_or_else(|| field.to_string())
}

fn probabilities(raw: &Map<String, Value>) -> Result<Vec<(&String, f64)>, String> {
    raw.get("probabilities")
        .and_then(Value::as_object)
        .ok_or_else(|| "probabilities".to_string())?
        .iter()
        .map(|(option, p)| {
            p.as_f64()
                .map(|p| (option, p))
                .ok_or_else(|| format!("probabilities.{option}"))
        })
        .collect()
}

// Score levels come keyed by their index as a string ("0", "1", ...).
fn index_keys<T>(
    map: &Map<String, Value>,
    field: &str,
    value: impl Fn(&Value) -> Option<T>,
) -> Result<BTreeMap<usize, T>, String> {
    map.iter()
        .map(|(key, v)| {
            let bad = || format!("{field}.{key}");
            let index = key.parse::<usize>().map_err(|_| bad())?;
            Ok((index, value(v).ok_or_else(bad)?))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Choice, Noul, Score};
    use serde_json::json;

    fn questions() -> Questions {
        Questions::new()
            .ask("billing", Noul::new("Billing?"))
            .ask(
                "tone",
                Choice::new("Tone?").options(["calm", "frustrated", "angry"]),
            )
            .ask(
                "urgency",
                Score::new("Urgent?", ["Can wait", "This week", "Today"]),
            )
            .ask("future", Question::Raw(json!({"type": "future"})))
    }

    fn answers() -> Value {
        json!({
            "billing": {"type": "noul", "noul": 0.99},
            "tone": {"type": "choice", "choice": "frustrated",
                     "probabilities": {"calm": 0.0, "frustrated": 0.86, "angry": 0.14},
                     "confidence": 0.78},
            "urgency": {"type": "score", "score": 1.99,
                        "legend": {"0": "Can wait", "1": "This week", "2": "Today"},
                        "probabilities": {"0": 0.0, "1": 0.01, "2": 0.99},
                        "confidence": 0.99},
            "future": {"type": "future", "whatever": [1, 2]}
        })
    }

    #[test]
    fn decodes_every_type() {
        let decoded = decode_all(&questions(), Some(&answers())).unwrap();

        let billing = decoded["billing"].as_noul().unwrap();
        assert!(billing.yes());
        assert!(!billing.yes_above(0.995));

        let tone = decoded["tone"].as_choice().unwrap();
        assert!(tone.is("frustrated"));
        assert_eq!(tone.probability("angry"), 0.14);
        assert_eq!(tone.probability("missing"), 0.0);
        assert_eq!(
            tone.ranked(),
            vec![("frustrated", 0.86), ("angry", 0.14), ("calm", 0.0)]
        );

        let urgency = decoded["urgency"].as_score().unwrap();
        assert_eq!(urgency.level(), 2);
        assert_eq!(urgency.legend[&1], json!("This week"));
        assert!((urgency.normalized() - 0.995).abs() < 1e-9);

        assert_eq!(
            decoded["future"].as_raw().unwrap(),
            &json!({"type": "future", "whatever": [1, 2]})
        );
    }

    #[test]
    fn names_the_first_bad_field() {
        let broken = |path: &str, value: Value| {
            let mut answers = answers();
            let (id, field) = path.split_once('.').unwrap();
            answers[id][field] = value;
            decode_all(&questions(), Some(&answers)).unwrap_err()
        };

        assert_eq!(
            broken("billing.noul", json!("high")),
            "answers.billing.noul"
        );
        assert_eq!(broken("tone.type", json!("noul")), "answers.tone.type");
        assert_eq!(
            broken("tone.confidence", Value::Null),
            "answers.tone.confidence"
        );
        assert_eq!(
            broken("tone.probabilities", json!({"calm": "x"})),
            "answers.tone.probabilities.calm"
        );
        assert_eq!(
            broken("urgency.legend", json!({"one": "x"})),
            "answers.urgency.legend.one"
        );

        let mut missing = answers();
        missing.as_object_mut().unwrap().remove("tone");
        assert_eq!(
            decode_all(&questions(), Some(&missing)).unwrap_err(),
            "answers.tone"
        );
        assert_eq!(decode_all(&questions(), None).unwrap_err(), "answers");
    }

    #[test]
    fn level_ties_go_to_the_lower_level() {
        let score = ScoreAnswer {
            score: 0.5,
            legend: BTreeMap::from([(0, json!("a")), (1, json!("b"))]),
            probabilities: BTreeMap::from([(0, 0.5), (1, 0.5)]),
            confidence: 0.0,
        };
        assert_eq!(score.level(), 0);
    }
}
