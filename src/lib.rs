//! Snapif.
#![forbid(unsafe_code)]

pub mod answer;
pub mod error;
pub mod ids;
mod macros;
pub mod policy;
pub mod question;
pub mod verdict;
pub mod wire;

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
