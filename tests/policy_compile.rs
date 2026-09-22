use snapif::error::PolicyError;
use snapif::ids::ActionId;
use snapif::policy::{HarmClass, NoulObs, Policy, UnsureVerdict, verdict_with_blocks};
use snapif::question::{ChoiceLabels, ScoreLabels};
use snapif::verdict::{Decision, Verdict};
use snapif::{choice, score};

choice! {
    enum Department {
        Billing = "billing" => "Payments, invoices, refunds",
        Technical = "technical" => "Bugs, outages, integrations",
        Sales = "sales" => "Pricing, upgrades",
    }
}

score! {
    enum Frustration {
        Calm = "Calm, just stating facts",
        Annoyed = "Frustrated but civil",
        Furious = "Angry, strong language",
    }
}

fn kind(action: &str, s: f64, nouls: &[NoulObs<'_>]) -> &'static str {
    let policy = Policy::shipped("tool-gate").unwrap();
    let verdict = verdict_with_blocks(&policy, &ActionId::new(action), s, None, nouls).unwrap();
    match verdict {
        Verdict::Auto(_) => "auto",
        Verdict::Review(_) => "review",
        Verdict::Escalate(_) => "escalate",
    }
}

#[test]
fn shipped_tool_gate_never_autos_git_push() {
    let policy = Policy::shipped("tool-gate").unwrap();
    let push = policy
        .actions
        .get(&ActionId::new("git.push"))
        .expect("git.push row");
    assert!(push.auto.is_none());
    let default = policy.default_action.as_ref().expect("default action");
    assert_eq!(default.review, 0.8);
    assert_eq!(default.class, HarmClass::Read);
    assert_eq!(default.when_unsure, UnsureVerdict::ReviewGuess);
}

#[test]
fn missing_unsure_when_escalate_below_is_zero() {
    let raw = "schema_version = 1\n[choice]\nescalate_below = 0.0\n";
    match Policy::from_toml_str(raw) {
        Err(PolicyError::MissingUnsure) => {}
        other => panic!("expected MissingUnsure, got {other:?}"),
    }
}

#[test]
fn default_action_is_required() {
    let raw = "schema_version = 1\n[choice]\nescalate_below = 0.8\n";
    match Policy::from_toml_str(raw) {
        Err(PolicyError::Invariant(message)) => assert!(message.contains("default_action")),
        other => panic!("expected missing default_action, got {other:?}"),
    }
}

#[test]
fn seven_numeric_examples() {
    assert_eq!(kind("tag", 0.85, &[]), "auto");
    assert_eq!(
        kind(
            "refund",
            0.75,
            &[NoulObs {
                id: "irreversible",
                p: 0.05
            }]
        ),
        "escalate"
    );
    assert_eq!(
        kind(
            "bash.rm",
            1.0,
            &[NoulObs {
                id: "destructive",
                p: 0.95
            }]
        ),
        "escalate"
    );
    assert_eq!(kind("git.push", 1.0, &[]), "review");
    assert_eq!(
        kind(
            "refund",
            0.99,
            &[NoulObs {
                id: "authority_claim",
                p: 0.50
            }]
        ),
        "escalate"
    );
    assert_eq!(kind("refund", 0.97, &[]), "review");
    assert_eq!(kind("tag", 0.97, &[]), "auto");
    assert_eq!(kind("tag", 0.70, &[]), "review");
}

#[test]
fn harm_class_bump_clears_auto() {
    let policy = Policy::shipped("tool-gate").unwrap();
    let gates =
        snapif::policy::effective_gates(&policy, &ActionId::new("tag"), Some(HarmClass::Write))
            .unwrap();
    assert!(gates.auto.is_none());
    assert_eq!(gates.harm_bumped, Some((HarmClass::Read, HarmClass::Write)));
}

#[test]
fn choice_and_score_labels() {
    assert_eq!(Department::Billing.as_label(), "billing");
    assert_eq!(
        Department::from_label("technical"),
        Some(Department::Technical)
    );
    assert_eq!(Frustration::Furious.as_index(), 2);
    assert_eq!(Frustration::from_index(0), Some(Frustration::Calm));
    let answer = snapif::answer::ChoiceAnswer {
        label: "billing".to_string(),
        confidence: 0.9,
        probabilities: indexmap::IndexMap::new(),
    };
    let known = answer.decide::<Department>(0.9, 0.8).unwrap();
    assert!(matches!(known, Decision::Known(Department::Billing)));
    let unsure = answer.decide::<Department>(0.2, 0.8).unwrap();
    assert!(matches!(unsure, Decision::Unsure { .. }));
}

#[test]
fn ui() {
    let tests = trybuild::TestCases::new();
    tests.compile_fail("tests/ui/decision_no_unsure.rs");
    // A `_` arm is exhaustive on stable rustc, so the footgun compiles.
    tests.pass("tests/ui/decision_wildcard.rs");
}
