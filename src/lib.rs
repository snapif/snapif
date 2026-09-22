//! Snapif.
#![forbid(unsafe_code)]

pub mod answer;
pub mod backend;
pub mod backends;
pub mod error;
pub mod ids;
mod macros;
pub mod policy;
pub mod question;
pub mod state;
pub mod usage;
pub mod verdict;
pub mod wire;

pub use backend::{AnswerMeta, AnyBackend, AskOut, CascadeHop, Client};
pub use backends::fake::FakeBackend;
pub use error::Error;
pub use ids::QuestionId;
pub use policy::Policy;
pub use question::Question;
pub use state::State;
pub use usage::UsageFn;
pub use verdict::{Decision, Verdict};

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
