//! Snapif.
#![forbid(unsafe_code)]

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
