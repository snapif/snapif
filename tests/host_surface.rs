use snapif::backend::Backend;
use snapif::error::{Error, PolicyError};
use snapif::policy::Fail;
use snapif::verdict::UnsureReason;
use snapif::{ActionHint, ActionId, Client, FakeBackend, Policy};

#[test]
fn normative_names_are_exported_and_from_error_marks_backend() {
    let from_policy = ActionHint::from_error(&PolicyError::MissingUnsure, ActionId::new("bash"));
    assert!(from_policy.guess.is_none());
    assert!(from_policy.meta.is_empty());
    assert!(matches!(
        from_policy.reasons.as_slice(),
        [UnsureReason::Backend { cause }] if cause.contains("escalate_below")
    ));
    let timeout = ActionHint::from_error(
        &Error::Timeout(std::time::Duration::from_millis(2000)),
        ActionId::new("bash"),
    );
    match timeout.reasons.as_slice() {
        [UnsureReason::Backend { cause }] => assert!(cause.contains("timeout"), "{cause}"),
        other => panic!("timeout reason: {other:?}"),
    }
    let long = "x".repeat(400);
    let rejected = ActionHint::from_error(
        &Error::Rejected {
            status: 422,
            body: long,
        },
        ActionId::new("bash"),
    );
    match rejected.reasons.as_slice() {
        [UnsureReason::Backend { cause }] => {
            assert!(cause.contains("422"), "{cause}");
            assert!(cause.ends_with("..."), "{cause}");
            assert!(cause.len() < 200, "{cause}");
        }
        other => panic!("rejected reason: {other:?}"),
    }
    let from_gate = ActionHint::from_error(&Error::EmptyActionId, ActionId::new("bash"));
    assert_eq!(from_gate.action_id.0, "bash");

    let client = Client::new(FakeBackend::new())
        .policy(Policy::shipped("tool-gate").expect("policy"))
        .fail(Fail::Closed);
    assert_eq!(client.backend().id(), "fake");
}
