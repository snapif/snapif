use indexmap::IndexMap;
use serde::Deserialize;
use serde_json::Value;

use crate::ids::QuestionId;
use crate::question::{ChoiceQ, NoulQ, Question};

const RAW: &str = include_str!("../policies/tool-gate.battery.json");

#[derive(Debug, Deserialize)]
struct BatteryFile {
    questions: Vec<BatteryQuestion>,
}

#[derive(Debug, Deserialize)]
struct BatteryQuestion {
    id: String,
    #[serde(rename = "type")]
    kind: String,
    instructions: String,
    #[serde(default)]
    criteria: Vec<String>,
}

pub fn shipped_questions() -> Vec<Question> {
    let file: BatteryFile = serde_json::from_str(RAW).expect("shipped battery json");
    file.questions
        .into_iter()
        .map(|question| match question.kind.as_str() {
            "noul" => Question::Noul(NoulQ {
                id: QuestionId::new(question.id),
                instructions: Value::String(question.instructions),
                criteria: None,
            }),
            "choice" => {
                let criteria = question
                    .criteria
                    .into_iter()
                    .map(|label| (label.clone(), Value::String(label)))
                    .collect::<IndexMap<_, _>>();
                Question::Choice(ChoiceQ {
                    id: QuestionId::new(question.id),
                    instructions: Value::String(question.instructions),
                    criteria,
                })
            }
            other => panic!("unknown battery type {other}"),
        })
        .collect()
}
