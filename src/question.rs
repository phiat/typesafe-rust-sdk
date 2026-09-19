//! Questions: the typed judgments to ask about a state.

use serde::ser::{Serialize, SerializeMap, Serializer};
use serde_json::Value;

/// A yes/no question. The answer, a [`NoulAnswer`](crate::NoulAnswer), is the
/// probability that the answer is yes.
///
/// ```
/// use typesafe::Noul;
///
/// let lights = Noul::new("If the user wants to change the lights, do they want them on?")
///     .yes("Lights on or brighter")
///     .no("Lights off or dimmer");
/// ```
#[derive(Clone, Debug, PartialEq)]
pub struct Noul {
    instructions: Value,
    yes: Option<Value>,
    no: Option<Value>,
}

impl Noul {
    /// `instructions` is a string, or a JSON object or array when structure
    /// makes the question clearer.
    pub fn new(instructions: impl Into<Value>) -> Self {
        Noul {
            instructions: instructions.into(),
            yes: None,
            no: None,
        }
    }

    /// What a yes means.
    pub fn yes(mut self, description: impl Into<Value>) -> Self {
        self.yes = Some(description.into());
        self
    }

    /// What a no means.
    pub fn no(mut self, description: impl Into<Value>) -> Self {
        self.no = Some(description.into());
        self
    }
}

/// A question that selects one option from a defined set. The answer is a
/// [`ChoiceAnswer`](crate::ChoiceAnswer).
///
/// Options are sent in the order they were added, because the order is part of
/// what the model reads. Include a no-match option when nothing may fit: the
/// model always picks one of the options given.
///
/// ```
/// use typesafe::Choice;
///
/// let team = Choice::new("Which team should own this ticket?")
///     .option("billing", "Charges, invoices, refunds")
///     .option("technical", "Bugs, outages, integrations")
///     .option("unclear", "The ticket does not say enough to tell");
///
/// let tone = Choice::new("What is the customer's tone?").options(["calm", "frustrated", "angry"]);
/// ```
#[derive(Clone, Debug, PartialEq)]
pub struct Choice {
    instructions: Value,
    options: Vec<(String, Value)>,
}

impl Choice {
    /// `instructions` is a string, or a JSON object or array when structure
    /// makes the question clearer.
    pub fn new(instructions: impl Into<Value>) -> Self {
        Choice {
            instructions: instructions.into(),
            options: Vec::new(),
        }
    }

    /// Adds an option with a description of when it applies.
    pub fn option(mut self, name: impl Into<String>, description: impl Into<Value>) -> Self {
        self.options.push((name.into(), description.into()));
        self
    }

    /// Adds options without descriptions, in order.
    pub fn options<I, S>(mut self, names: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.options
            .extend(names.into_iter().map(|name| (name.into(), Value::Null)));
        self
    }

    /// The option names, in the order they are sent.
    pub fn option_names(&self) -> impl Iterator<Item = &str> {
        self.options.iter().map(|(name, _)| name.as_str())
    }
}

/// A question that rates the state against ordered levels, worst or least
/// first. The answer is a [`ScoreAnswer`](crate::ScoreAnswer).
///
/// Each level should describe a concrete situation on its own:
///
/// ```
/// use typesafe::Score;
///
/// let urgency = Score::new(
///     "How urgently does this ticket need a response?",
///     [
///         "Can wait: a question or feedback, nothing is blocked",
///         "Soon: a problem with a workaround",
///         "Today: something important is broken for the customer",
///     ],
/// );
/// ```
#[derive(Clone, Debug, PartialEq)]
pub struct Score {
    instructions: Value,
    levels: Vec<Value>,
}

impl Score {
    /// At least two levels, worst or least first.
    pub fn new<I, L>(instructions: impl Into<Value>, levels: I) -> Self
    where
        I: IntoIterator<Item = L>,
        L: Into<Value>,
    {
        Score {
            instructions: instructions.into(),
            levels: levels.into_iter().map(Into::into).collect(),
        }
    }
}

/// Any one question.
///
/// [`Question::Raw`] holds a question as its JSON, sent as given. Its answer
/// comes back as [`Answer::Raw`](crate::Answer::Raw). That covers question types
/// this client predates.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum Question {
    Noul(Noul),
    Choice(Choice),
    Score(Score),
    Raw(Value),
}

impl From<Noul> for Question {
    fn from(q: Noul) -> Self {
        Question::Noul(q)
    }
}

impl From<Choice> for Question {
    fn from(q: Choice) -> Self {
        Question::Choice(q)
    }
}

impl From<Score> for Question {
    fn from(q: Score) -> Self {
        Question::Score(q)
    }
}

/// The questions for one call, each under an id. Answers come back under the
/// same ids.
///
/// Ids are never sent to the model as meaning, so each question has to carry
/// its own. Questions in one call share one read of the state and are answered
/// in parallel, without seeing each other's answers.
///
/// ```
/// use typesafe::{Choice, Noul, Questions};
///
/// let questions = Questions::new()
///     .ask("billing", Noul::new("Is this ticket about billing?"))
///     .ask("tone", Choice::new("What is the customer's tone?").options(["calm", "angry"]));
/// assert_eq!(questions.len(), 2);
/// ```
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Questions(Vec<(String, Question)>);

impl Questions {
    pub fn new() -> Self {
        Questions(Vec::new())
    }

    /// Adds a question, builder style.
    pub fn ask(mut self, id: impl Into<String>, question: impl Into<Question>) -> Self {
        self.insert(id, question);
        self
    }

    /// Adds a question, for building in a loop.
    pub fn insert(&mut self, id: impl Into<String>, question: impl Into<Question>) -> &mut Self {
        self.0.push((id.into(), question.into()));
        self
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn get(&self, id: &str) -> Option<&Question> {
        self.0.iter().find(|(k, _)| k == id).map(|(_, q)| q)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &Question)> {
        self.0.iter().map(|(k, q)| (k.as_str(), q))
    }

    /// Checks the questions before anything is sent. The error message names
    /// the question at fault.
    pub(crate) fn validate(&self) -> Result<(), String> {
        if self.0.is_empty() {
            return Err("questions must not be empty".into());
        }
        for (i, (id, question)) in self.0.iter().enumerate() {
            if id.is_empty() {
                return Err("question ids must not be empty".into());
            }
            if self.0[..i].iter().any(|(other, _)| other == id) {
                return Err(format!("question ids must be unique, got {id:?} twice"));
            }
            validate_question(question).map_err(|problem| format!("question {id:?}: {problem}"))?;
        }
        Ok(())
    }
}

impl<K: Into<String>, Q: Into<Question>> FromIterator<(K, Q)> for Questions {
    fn from_iter<T: IntoIterator<Item = (K, Q)>>(iter: T) -> Self {
        Questions(
            iter.into_iter()
                .map(|(k, q)| (k.into(), q.into()))
                .collect(),
        )
    }
}

impl<K: Into<String>, Q: Into<Question>> Extend<(K, Q)> for Questions {
    fn extend<T: IntoIterator<Item = (K, Q)>>(&mut self, iter: T) {
        self.0
            .extend(iter.into_iter().map(|(k, q)| (k.into(), q.into())));
    }
}

fn validate_question(question: &Question) -> Result<(), String> {
    match question {
        Question::Noul(q) => {
            check_instructions(&q.instructions)?;
            for description in q.yes.iter().chain(&q.no) {
                check_description(description, "Noul criteria")?;
            }
        }
        Question::Choice(q) => {
            check_instructions(&q.instructions)?;
            if q.options.is_empty() {
                return Err("a Choice needs at least one option".into());
            }
            for (i, (name, description)) in q.options.iter().enumerate() {
                if name.is_empty() {
                    return Err("Choice options must be non-empty strings".into());
                }
                if q.options[..i].iter().any(|(other, _)| other == name) {
                    return Err(format!("Choice options must be unique, got {name:?} twice"));
                }
                check_description(description, "Choice option")?;
            }
        }
        Question::Score(q) => {
            check_instructions(&q.instructions)?;
            if q.levels.len() < 2 {
                return Err(format!(
                    "a Score needs at least two levels, got {}",
                    q.levels.len()
                ));
            }
            for level in &q.levels {
                if level.is_null() {
                    return Err("Score levels must describe something, got null".into());
                }
                check_description(level, "Score level")?;
            }
        }
        Question::Raw(raw) => {
            if !raw.get("type").is_some_and(Value::is_string) {
                return Err("a raw question must be a JSON object with a \"type\" string".into());
            }
        }
    }
    Ok(())
}

fn check_instructions(instructions: &Value) -> Result<(), String> {
    let valid = match instructions {
        Value::String(s) => !s.trim().is_empty(),
        Value::Object(o) => !o.is_empty(),
        Value::Array(a) => !a.is_empty(),
        _ => false,
    };
    if valid {
        Ok(())
    } else {
        Err(format!(
            "instructions must be a non-empty string, object or array, got {instructions}"
        ))
    }
}

fn check_description(description: &Value, what: &str) -> Result<(), String> {
    match description {
        Value::String(_) | Value::Object(_) | Value::Array(_) | Value::Null => Ok(()),
        other => Err(format!(
            "{what} descriptions must be strings, objects, arrays or null, got {other}"
        )),
    }
}

// Wire encoding. Questions and Choice options are written as JSON objects in
// insertion order, which a serde_json::Value would sort.

impl Serialize for Questions {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(self.0.len()))?;
        for (id, question) in &self.0 {
            map.serialize_entry(id, question)?;
        }
        map.end()
    }
}

impl Serialize for Question {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Question::Noul(q) => {
                let criteria = q.yes.is_some() || q.no.is_some();
                let mut map = serializer.serialize_map(Some(2 + criteria as usize))?;
                map.serialize_entry("type", "noul")?;
                map.serialize_entry("instructions", &q.instructions)?;
                if criteria {
                    map.serialize_entry("criteria", &NoulCriteria(q))?;
                }
                map.end()
            }
            Question::Choice(q) => {
                let mut map = serializer.serialize_map(Some(3))?;
                map.serialize_entry("type", "choice")?;
                map.serialize_entry("instructions", &q.instructions)?;
                map.serialize_entry("criteria", &OrderedOptions(&q.options))?;
                map.end()
            }
            Question::Score(q) => {
                let mut map = serializer.serialize_map(Some(3))?;
                map.serialize_entry("type", "score")?;
                map.serialize_entry("instructions", &q.instructions)?;
                map.serialize_entry("criteria", &q.levels)?;
                map.end()
            }
            Question::Raw(raw) => raw.serialize(serializer),
        }
    }
}

struct NoulCriteria<'a>(&'a Noul);

impl Serialize for NoulCriteria<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(None)?;
        if let Some(yes) = &self.0.yes {
            map.serialize_entry("true", yes)?;
        }
        if let Some(no) = &self.0.no {
            map.serialize_entry("false", no)?;
        }
        map.end()
    }
}

struct OrderedOptions<'a>(&'a [(String, Value)]);

impl Serialize for OrderedOptions<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(self.0.len()))?;
        for (name, description) in self.0 {
            map.serialize_entry(name, description)?;
        }
        map.end()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn wire(questions: &Questions) -> String {
        serde_json::to_string(questions).unwrap()
    }

    #[test]
    fn encodes_each_type() {
        let questions = Questions::new()
            .ask("b", Noul::new("Is it billing?"))
            .ask("l", Noul::new("On?").yes("Lights on").no("Lights off"))
            .ask("t", Choice::new("Tone?").options(["calm", "angry"]))
            .ask("u", Score::new("Urgent?", ["Can wait", "Today"]))
            .ask("r", Question::Raw(json!({"type": "future", "x": 1})));
        assert_eq!(
            serde_json::from_str::<Value>(&wire(&questions)).unwrap(),
            json!({
                "b": {"type": "noul", "instructions": "Is it billing?"},
                "l": {"type": "noul", "instructions": "On?",
                      "criteria": {"true": "Lights on", "false": "Lights off"}},
                "t": {"type": "choice", "instructions": "Tone?",
                      "criteria": {"calm": null, "angry": null}},
                "u": {"type": "score", "instructions": "Urgent?", "criteria": ["Can wait", "Today"]},
                "r": {"type": "future", "x": 1},
            })
        );
    }

    #[test]
    fn keeps_choice_option_order() {
        let questions = Questions::new().ask(
            "q",
            Choice::new("Which?")
                .option("zebra", "last alphabetically")
                .options(["mango", "apple"]),
        );
        assert_eq!(
            wire(&questions),
            r#"{"q":{"type":"choice","instructions":"Which?","criteria":{"zebra":"last alphabetically","mango":null,"apple":null}}}"#
        );
    }

    #[test]
    fn structured_instructions_and_descriptions() {
        let questions = Questions::new().ask(
            "q",
            Choice::new(json!({"task": "Pick one", "rules": ["be literal"]}))
                .option("a", json!({"when": "always"})),
        );
        assert!(questions.validate().is_ok());
        assert!(wire(&questions).contains(r#""criteria":{"a":{"when":"always"}}"#));
    }

    #[test]
    fn only_given_noul_criteria_are_sent() {
        let questions = Questions::new().ask("q", Noul::new("Is it?").yes("It is"));
        assert!(wire(&questions).contains(r#""criteria":{"true":"It is"}"#));
    }

    #[test]
    fn validation_names_the_problem() {
        let problem = |questions: Questions| questions.validate().unwrap_err();

        assert_eq!(problem(Questions::new()), "questions must not be empty");
        assert!(problem(Questions::new().ask("q", Noul::new("  "))).contains("instructions"));
        assert!(problem(Questions::new().ask("q", Noul::new(3))).contains("instructions"));
        assert!(
            problem(Questions::new().ask("q", Choice::new("Which?")))
                .contains("at least one option")
        );
        assert!(
            problem(Questions::new().ask("q", Choice::new("Which?").options(["a", "a"])))
                .contains("unique")
        );
        assert!(
            problem(Questions::new().ask("q", Choice::new("Which?").option("a", 1)))
                .contains("descriptions")
        );
        assert!(
            problem(Questions::new().ask("q", Score::new("How?", ["only one"])))
                .contains("at least two levels")
        );
        assert!(
            problem(Questions::new().ask("q", Question::Raw(json!({"no": "type"}))))
                .contains("\"type\"")
        );
        let dup = Questions::new()
            .ask("q", Noul::new("One?"))
            .ask("q", Noul::new("Two?"));
        assert!(problem(dup).contains("unique"));
        assert!(
            Questions::new()
                .ask("q", Noul::new("Is it?"))
                .validate()
                .is_ok()
        );
    }

    #[test]
    fn collects_from_pairs() {
        let questions: Questions = (1..=3)
            .map(|i| {
                (
                    format!("claim_{i}"),
                    Noul::new(format!("Is claim {i} supported?")),
                )
            })
            .collect();
        assert_eq!(questions.len(), 3);
        assert!(questions.get("claim_2").is_some());
        assert!(questions.validate().is_ok());
    }
}
