use serde::Deserialize;
use serde_json::Value;

use crate::ids::QuestionId;
use crate::question::{NoulQ, Question};

const RAW: &str = include_str!("../policies/screen.questions.json");

#[derive(Debug, Deserialize)]
struct QuestionFile {
    questions: Vec<RawQuestion>,
}

#[derive(Debug, Deserialize)]
struct RawQuestion {
    id: String,
    #[serde(rename = "type")]
    kind: String,
    instructions: String,
}

pub fn questions() -> Vec<Question> {
    let file: QuestionFile = serde_json::from_str(RAW).expect("shipped screen questions");
    file.questions
        .into_iter()
        .map(|question| {
            assert_eq!(question.kind, "noul", "screen ships only noul questions");
            Question::Noul(NoulQ {
                id: QuestionId::new(question.id),
                instructions: Value::String(question.instructions),
                criteria: None,
            })
        })
        .collect()
}
