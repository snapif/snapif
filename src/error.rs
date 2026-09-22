use crate::ids::{ActionId, QuestionId};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum WireError {
    #[error("unknown question/answer type {0}")]
    UnknownType(String),
    #[error("invalid json: {0}")]
    Json(String),
    #[error("empty choice criteria")]
    EmptyChoice,
    #[error("choice has more than 255 options")]
    ChoiceTooWide,
    #[error("score criteria must be 2..=10, got {0}")]
    ScoreLen(usize),
    #[error("body exceeds snapif encode cap ({0} bytes)")]
    BodyCap(usize),
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum DecodeError {
    #[error("unknown choice label {label} for {key}")]
    UnknownLabel { key: QuestionId, label: String },
    #[error("out of range on {key}")]
    OutOfRange { key: QuestionId },
    #[error("answer type mismatch on {key}")]
    TypeMismatch { key: QuestionId },
    #[error("missing answer {key}")]
    MissingAnswer { key: QuestionId },
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum PolicyError {
    #[error("unknown schema_version {0}")]
    Schema(u32),
    #[error("missing unsure path: escalate_below must be > 0")]
    MissingUnsure,
    #[error("threshold invariant violated: {0}")]
    Invariant(String),
    /// `SNAPIF_BACKEND` is missing or not a known name. Not a gate threshold.
    #[error("{0}")]
    BackendName(String),
    /// A setting or policy file is wrong. Not a gate threshold.
    #[error("{0}")]
    Config(String),
    #[error("unknown action {0} and no default_action")]
    UnknownAction(ActionId),
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum BackendError {
    #[error("auth: authentication failed")]
    Auth,
    #[error("timeout: the deadline was exceeded")]
    Timeout,
    #[error("rate limited")]
    RateLimit,
    #[error("overloaded")]
    Overloaded,
    #[error("rejected HTTP {status}: {body}")]
    Rejected { status: u16, body: String },
    #[error("{0}")]
    Transport(String),
}

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
    #[error("auth: {0}")]
    Auth(String),
    #[error("wire: {0}")]
    Wire(#[from] WireError),
    #[error("decode: {0}")]
    Decode(#[from] DecodeError),
    #[error("policy: {0}")]
    Policy(#[from] PolicyError),
    #[error("timeout after {0:?}")]
    Timeout(std::time::Duration),
    #[error("rate limited")]
    RateLimit,
    #[error("backend overloaded")]
    Overloaded,
    #[error("rejected HTTP {status}: {body}")]
    Rejected { status: u16, body: String },
    #[error("backend: {0}")]
    Backend(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("empty action_id")]
    EmptyActionId,
}
