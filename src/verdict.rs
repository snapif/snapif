use crate::error::DecodeError;
use crate::ids::{ActionId, QuestionId};
use crate::policy::{BlockWhen, HarmClass};
use crate::wire::Usage;

#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum UnsureReason {
    BelowFloor { confidence: f64, floor: f64 },
    BelowAuto { confidence: f64, auto: f64 },
    NoulBand { noul: f64 },
    Battery { id: QuestionId, when: BlockWhen },
    AuthorityClaim { noul: f64 },
    Decode(DecodeError),
    Wire,
    Backend,
    CascadeStillUnsure,
    HarmClassBump { from: HarmClass, to: HarmClass },
    Truncated,
}

#[must_use]
#[derive(Debug, Clone, PartialEq)]
pub enum Decision<T> {
    Known(T),
    Unsure {
        reason: UnsureReason,
        guess: Option<T>,
    },
}

#[must_use]
#[derive(Debug, Clone, PartialEq)]
pub enum Verdict {
    Auto(ActionHint),
    Review(ActionHint),
    Escalate(ActionHint),
}

#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct ActionHint {
    pub action_id: ActionId,
    pub guess: Option<String>,
    pub reasons: Vec<UnsureReason>,
    pub shadow: bool,
    pub usage: Usage,
    pub backend_id: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum UntypedDecision {
    Choice(Decision<String>),
    Score(Decision<f64>),
    Noul(Decision<bool>),
}

pub(crate) fn hint(action_id: ActionId, reasons: Vec<UnsureReason>) -> ActionHint {
    ActionHint {
        action_id,
        guess: None,
        reasons,
        shadow: false,
        usage: Usage::default(),
        backend_id: String::new(),
    }
}
