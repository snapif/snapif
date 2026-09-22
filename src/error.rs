use crate::ids::QuestionId;
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
pub enum BackendError {
    #[error("auth")]
    Auth,
    #[error("timeout")]
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
