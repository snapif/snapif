//! Snapif.
#![forbid(unsafe_code)]

pub mod answer;
pub mod backend;
pub mod backends;
pub mod battery;
pub mod error;
pub mod gate;
pub mod ids;
mod macros;
pub mod policy;
pub mod question;
pub mod review;
pub mod scorecard;
pub mod screen;
pub mod state;
pub mod triage;
pub mod usage;
pub mod verdict;
pub mod wire;

pub use backend::{AnswerMeta, AnyBackend, AskOut, CascadeHop, Client, ClientConfig};
pub use backends::fake::FakeBackend;
pub use error::Error;
pub use gate::GateRequest;
pub use ids::{ActionId, QuestionId};
pub use policy::{Fail, Policy};
pub use question::Question;
pub use state::{PreparedCall, State};
pub use usage::UsageFn;
pub use verdict::{ActionHint, Decision, GateFacts, Verdict};

#[cfg(test)]
mod tests {
    #[test]
    fn forbids_unsafe_code() {
        let src = include_str!("lib.rs");
        assert!(
            src.contains("forbid(unsafe_code)"),
            "crate root must forbid unsafe code"
        );
    }
}
