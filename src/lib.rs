//! Snapif scores one tool call and returns Auto, Review, or Escalate.
//!
//! The library does not execute the tool. [`Policy::shipped`]`("tool-gate")`
//! is the tool gate. `review` and `screen` are ask-only and cannot return
//! Auto. Build a [`Client`] with [`FakeBackend`] for tests, or use the
//! `typesafe` or `compatible` HTTP backends. The default model on the wire
//! is `jev-latest`.
//!
//! Default features are empty. The `cli` feature builds the `snapif` binary
//! and includes HTTP.
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

pub use backend::{
    AnswerMeta, AnyBackend, AskOut, CallChoice, CascadeHop, Client, ClientConfig, ClientStatus,
};
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
