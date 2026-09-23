use std::error::Error;

use indexmap::IndexMap;

use crate::error::DecodeError;
use crate::ids::{ActionId, QuestionId};
use crate::policy::{BlockWhen, HarmClass};
use crate::wire::Usage;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CascadeHop {
    First,
    Fallback,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct AnswerMeta {
    pub original_prob_sum: Option<f64>,
    pub cascade_hop: Option<CascadeHop>,
    pub first_hop_error: bool,
}

#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum UnsureReason {
    BelowFloor {
        confidence: f64,
        floor: f64,
    },
    BelowAuto {
        confidence: f64,
        auto: f64,
    },
    /// Cleared the review floor and did not clear an auto band.
    /// `auto` is `None` when the action has no auto threshold.
    ReviewFloor {
        confidence: f64,
        floor: f64,
        auto: Option<f64>,
    },
    NoulBand {
        noul: f64,
    },
    Battery {
        id: QuestionId,
        when: BlockWhen,
    },
    AuthorityClaim {
        noul: f64,
    },
    Decode(DecodeError),
    Wire,
    /// The scorer failed. `cause` is a short timeout, status, or body.
    Backend {
        cause: String,
    },
    CascadeStillUnsure,
    HarmClassBump {
        from: HarmClass,
        to: HarmClass,
    },
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
    pub meta: IndexMap<String, AnswerMeta>,
}

impl ActionHint {
    /// Hint a host can show when policy load or `gate` fails before a verdict.
    ///
    /// The reason is [`UnsureReason::Backend`] and its `cause` is `error`'s text.
    /// Policy errors and [`crate::error::Error`] both implement [`std::error::Error`].
    pub fn from_error(error: &dyn Error, action_id: ActionId) -> Self {
        hint(action_id, vec![backend_cause(error)])
    }
}

/// Clip a scorer failure so a host line stays short.
pub fn backend_cause(error: &dyn Error) -> UnsureReason {
    const MAX: usize = 160;
    let text = error.to_string();
    let cause = if text.len() <= MAX {
        text
    } else {
        let mut end = MAX;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}...", &text[..end])
    };
    UnsureReason::Backend { cause }
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::fmt;

    use super::backend_cause;
    use crate::verdict::UnsureReason;

    #[derive(Debug)]
    struct Msg(String);

    impl fmt::Display for Msg {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str(&self.0)
        }
    }

    impl Error for Msg {}

    #[test]
    fn backend_cause_keeps_a_short_timeout() {
        let reason = backend_cause(&Msg("timeout: the deadline was exceeded".to_owned()));
        assert!(matches!(
            reason,
            UnsureReason::Backend { cause } if cause == "timeout: the deadline was exceeded"
        ));
    }

    #[test]
    fn backend_cause_clips_a_long_body() {
        let reason = backend_cause(&Msg("x".repeat(400)));
        let UnsureReason::Backend { cause } = reason else {
            panic!("expected backend");
        };
        assert!(cause.ends_with("..."), "{cause}");
        assert!(cause.len() <= 163, "{}", cause.len());
    }
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
        meta: IndexMap::new(),
    }
}
