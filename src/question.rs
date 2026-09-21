use crate::ids::QuestionId;
use crate::wire::NoulCriteria;
use indexmap::IndexMap;
use serde_json::Value;

pub struct ChoiceQ {
    pub id: QuestionId,
    pub instructions: Value,
    pub criteria: IndexMap<String, Value>,
}

pub struct ScoreQ {
    pub id: QuestionId,
    pub instructions: Value,
    pub criteria: Vec<Value>,
}

pub struct NoulQ {
    pub id: QuestionId,
    pub instructions: Value,
    pub criteria: Option<NoulCriteria>,
}

pub enum Question {
    Choice(ChoiceQ),
    Score(ScoreQ),
    Noul(NoulQ),
}

pub trait ChoiceLabels: Sized {
    fn labels() -> &'static [(&'static str, &'static str)];
    fn from_label(s: &str) -> Option<Self>;
    fn as_label(&self) -> &'static str;
}

pub trait ScoreLabels: Sized {
    fn criteria() -> &'static [&'static str];
    fn from_index(i: usize) -> Option<Self>;
    fn as_index(&self) -> usize;
}
