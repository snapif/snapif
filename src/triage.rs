use indexmap::IndexMap;
use serde::Deserialize;
use serde_json::Value;

use crate::ids::QuestionId;
use crate::question::{ChoiceQ, NoulQ, Question};

const RAW: &str = include_str!("../policies/triage.questions.json");

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
    #[serde(default)]
    criteria: Vec<RawCriterion>,
}

#[derive(Debug, Deserialize)]
struct RawCriterion {
    label: String,
    description: String,
}

pub fn questions() -> Vec<Question> {
    let file: QuestionFile = serde_json::from_str(RAW).expect("shipped triage questions");
    file.questions
        .into_iter()
        .map(|question| match question.kind.as_str() {
            "noul" => {
                assert!(
                    question.criteria.is_empty(),
                    "triage noul {} has criteria",
                    question.id
                );
                Question::Noul(NoulQ {
                    id: QuestionId::new(question.id),
                    instructions: Value::String(question.instructions),
                    criteria: None,
                })
            }
            "choice" => {
                assert!(
                    !question.criteria.is_empty(),
                    "triage choice {} has no criteria",
                    question.id
                );
                let criteria = question
                    .criteria
                    .into_iter()
                    .map(|row| (row.label, Value::String(row.description)))
                    .collect::<IndexMap<_, _>>();
                Question::Choice(ChoiceQ {
                    id: QuestionId::new(question.id),
                    instructions: Value::String(question.instructions),
                    criteria,
                })
            }
            other => panic!("unknown triage question type {other}"),
        })
        .collect()
}
