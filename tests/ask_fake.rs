use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use indexmap::IndexMap;
use serde_json::{Value, json};
use snapif::backend::Backend;
use snapif::error::{BackendError, DecodeError};
use snapif::question::{ChoiceLabels, ChoiceQ, NoulQ, ScoreLabels, ScoreQ};
use snapif::verdict::UnsureReason;
use snapif::wire::{ENCODE_CAP, WireQuestion, WireRequest};
use snapif::{Client, Decision, Error, FakeBackend, Policy, Question, QuestionId, State};

snapif::choice! {
    enum Department {
        Billing = "billing" => "Payments, invoices, refunds",
        Technical = "technical" => "Bugs, outages, integrations",
        Sales = "sales" => "Pricing, upgrades",
    }
}

snapif::score! {
    enum Frustration {
        Calm = "Calm, just stating facts",
        Annoyed = "Frustrated but civil",
        Furious = "Angry, strong language",
    }
}

fn policy() -> Policy {
    Policy::shipped("tool-gate").expect("shipped tool-gate")
}

fn state() -> State {
    State {
        trusted: json!({"user_request": "invoice"}),
        untrusted: Value::Null,
    }
}

fn department() -> Question {
    let mut criteria = IndexMap::new();
    for (id, text) in Department::labels() {
        criteria.insert((*id).to_string(), Value::String((*text).to_string()));
    }
    Question::Choice(ChoiceQ {
        id: QuestionId::new("department"),
        instructions: json!("Which department owns this request?"),
        criteria,
    })
}

fn frustration() -> Question {
    Question::Score(ScoreQ {
        id: QuestionId::new("frustration"),
        instructions: json!("How frustrated is the customer?"),
        criteria: Frustration::criteria()
            .iter()
            .map(|text| Value::String((*text).to_string()))
            .collect(),
    })
}

fn noul(id: &str) -> Question {
    Question::Noul(NoulQ {
        id: QuestionId::new(id),
        instructions: json!("Will this prepared call be hard to undo?"),
        criteria: None,
    })
}

#[test]
fn ask_known_choice_reports_usage_and_state() {
    let seen = Arc::new(Mutex::new(None));
    let slot = Arc::clone(&seen);
    let backend = FakeBackend::new().on_choice("department", "billing", 0.91);
    let client = Client::new(backend)
        .policy(policy())
        .on_usage(Arc::new(move |usage| {
            *slot.lock().expect("usage") = Some(usage);
        }));
    let out = pollster::block_on(client.ask(state(), vec![department()])).expect("ask");
    let decision = out
        .choice::<Department>(&QuestionId::new("department"))
        .expect("choice");
    assert!(matches!(decision, Decision::Known(Department::Billing)));
    let usage = seen.lock().expect("usage").expect("callback");
    assert_eq!(usage.input_tokens, 0);
    assert_eq!(usage.output_tokens, 0);
    let recorded = client.backend().last_state().expect("state");
    assert_eq!(recorded["trusted"]["user_request"], "invoice");
    assert!(recorded.get("prepared").is_none());
}

#[test]
fn low_confidence_is_unsure() {
    let backend = FakeBackend::new().on_choice("department", "billing", 0.5);
    let client = Client::new(backend).policy(policy());
    let out = pollster::block_on(client.ask(state(), vec![department()])).expect("ask");
    let decision = out
        .choice::<Department>(&QuestionId::new("department"))
        .expect("choice");
    assert!(matches!(
        decision,
        Decision::Unsure {
            reason: UnsureReason::BelowFloor { .. },
            guess: Some(Department::Billing),
        }
    ));
}

#[test]
fn unscripted_id_is_missing_answer() {
    let client = Client::new(FakeBackend::new()).policy(policy());
    let err = pollster::block_on(client.ask(state(), vec![department()])).expect_err("missing");
    assert!(matches!(
        err,
        Error::Decode(DecodeError::MissingAnswer { .. })
    ));
}

#[test]
fn scripted_timeout_does_not_wait_for_deadline() {
    let backend = FakeBackend::new().on_timeout("department");
    let client = Client::new(backend).policy(policy());
    let err = pollster::block_on(client.ask(state(), vec![department()])).expect_err("timeout");
    assert!(matches!(err, Error::Timeout(_)));
}

#[test]
fn past_deadline_is_timeout() {
    let backend = FakeBackend::new().on_choice("department", "billing", 0.91);
    let mut criteria = IndexMap::new();
    criteria.insert("billing".to_string(), json!("Payments"));
    let mut questions = IndexMap::new();
    questions.insert(
        "department".to_string(),
        WireQuestion::Choice {
            instructions: json!("dept"),
            criteria,
        },
    );
    let request = WireRequest {
        model: "jev-latest".to_string(),
        state: json!({}),
        questions,
    };
    let err =
        pollster::block_on(backend.evaluate(request, Instant::now() - Duration::from_secs(1)))
            .expect_err("past");
    assert!(matches!(err, BackendError::Timeout));
}

#[test]
fn noul_known_and_band() {
    let high = Client::new(FakeBackend::new().on_noul("irreversible", 0.95)).policy(policy());
    let high = pollster::block_on(high.ask(state(), vec![noul("irreversible")])).expect("ask");
    assert!(matches!(
        high.noul(&QuestionId::new("irreversible")).expect("noul"),
        Decision::Known(true)
    ));
    let mid = Client::new(FakeBackend::new().on_noul("irreversible", 0.5)).policy(policy());
    let mid = pollster::block_on(mid.ask(state(), vec![noul("irreversible")])).expect("ask");
    assert!(matches!(
        mid.noul(&QuestionId::new("irreversible")).expect("noul"),
        Decision::Unsure {
            reason: UnsureReason::NoulBand { noul },
            guess: Some(true),
        } if (noul - 0.5).abs() < 1e-9
    ));
}

#[test]
fn score_maps_to_level() {
    let client = Client::new(FakeBackend::new().on_score("frustration", 1.0)).policy(policy());
    let out = pollster::block_on(client.ask(state(), vec![frustration()])).expect("ask");
    assert!(matches!(
        out.score::<Frustration>(&QuestionId::new("frustration"))
            .expect("score"),
        Decision::Known(Frustration::Annoyed)
    ));
}

#[test]
fn ask_reports_truncated_untrusted_from_the_wire_cap() {
    let client =
        Client::new(FakeBackend::new().on_choice("department", "billing", 0.91)).policy(policy());
    let small = pollster::block_on(client.ask(state(), vec![department()])).expect("small");
    assert!(!small.truncated_untrusted);
    assert!(matches!(
        small
            .choice::<Department>(&QuestionId::new("department"))
            .expect("choice"),
        Decision::Known(Department::Billing)
    ));

    let huge = State {
        trusted: json!({"user_request": "invoice"}),
        untrusted: json!("u".repeat(ENCODE_CAP)),
    };
    let out = pollster::block_on(client.ask(huge, vec![department()])).expect("truncated");
    assert!(out.truncated_untrusted);
    assert!(matches!(
        out.choice::<Department>(&QuestionId::new("department"))
            .expect("choice"),
        Decision::Known(Department::Billing)
    ));
}
