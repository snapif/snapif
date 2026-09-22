use indexmap::IndexMap;
use serde_json::{Value, json};
use snapif::policy::{HarmClass, UnsureVerdict};
use snapif::state::{PreparedCall, State};
use snapif::wire::{WireQuestion, WireRequest};
use snapif::{
    Client, Decision, FakeBackend, GateRequest, Policy, Question, QuestionId, Verdict, battery,
};

snapif::choice! {
    enum Department {
        Billing = "billing" => "Payments, invoices, refunds",
        Technical = "technical" => "Bugs, outages, integrations",
        Sales = "sales" => "Pricing, upgrades",
    }
}

fn state() -> State {
    State {
        trusted: json!({"user_request": "invoice"}),
        untrusted: Value::Null,
    }
}

fn triage() -> Policy {
    Policy::shipped("triage").expect("shipped triage")
}

fn client(backend: FakeBackend) -> Client<FakeBackend> {
    Client::new(backend).policy(triage())
}

fn ask(backend: FakeBackend) -> snapif::AskOut {
    pollster::block_on(client(backend).ask(state(), snapif::triage::questions())).expect("ask")
}

fn wire_questions(questions: Vec<Question>) -> IndexMap<String, WireQuestion> {
    questions
        .into_iter()
        .map(|question| match question {
            Question::Choice(choice) => (
                choice.id.0,
                WireQuestion::Choice {
                    instructions: choice.instructions,
                    criteria: choice.criteria,
                },
            ),
            Question::Noul(noul) => (
                noul.id.0,
                WireQuestion::Noul {
                    instructions: noul.instructions,
                    criteria: noul.criteria,
                },
            ),
            Question::Score(_) => panic!("triage ships no score question"),
        })
        .collect()
}

#[test]
fn shipped_triage_is_sealed_observation() {
    let policy = triage();
    assert_eq!(policy.fail, snapif::Fail::Closed);
    assert_eq!(policy.cascade_min, 0.8);
    assert_eq!(policy.battery.0, "triage");
    assert_eq!(policy.choice.escalate_below, 0.8);
    assert_eq!(policy.choice.review_below, 1.0);
    assert!(policy.actions.is_empty());
    let default = policy.default_action.expect("default action");
    assert!(default.auto.is_none());
    assert!(default.block_on.is_empty());
    assert_eq!(default.review, 0.8);
    assert_eq!(default.class, HarmClass::Read);
    assert_eq!(default.when_unsure, UnsureVerdict::Escalate);
    let out = ask(FakeBackend::new()
        .on_choice("department", "billing", 0.91)
        .on_noul("wants_refund", 0.05));
    assert_eq!(
        out.choice::<Department>(&QuestionId::new("department"))
            .expect("department"),
        Decision::Known(Department::Billing)
    );
}

#[test]
fn unknown_shipped_policy_is_still_an_error() {
    let err = Policy::shipped("nope").expect_err("unknown policy");
    let message = err.to_string();
    assert!(message.contains("nope"), "{message}");
}

#[test]
fn shipped_questions_round_trip() {
    let questions = snapif::triage::questions();
    assert_eq!(questions.len(), 2);
    let request = WireRequest {
        model: "jev-latest".to_string(),
        state: state().to_wire(None),
        questions: wire_questions(questions),
    };
    let encoded = snapif::wire::encode(&request).expect("encode");
    let decoded = snapif::wire::decode_request(&encoded.body).expect("decode");
    let ids: Vec<_> = decoded.questions.keys().cloned().collect();
    assert_eq!(ids, ["department", "wants_refund"]);
    match &decoded.questions["department"] {
        WireQuestion::Choice {
            instructions,
            criteria,
        } => {
            assert_eq!(instructions, &json!("Which department owns this request?"));
            let labels: Vec<_> = criteria.keys().cloned().collect();
            assert_eq!(labels, ["billing", "technical", "sales"]);
            assert_eq!(criteria["billing"], json!("Payments, invoices, refunds"));
            assert_eq!(criteria["technical"], json!("Bugs, outages, integrations"));
            assert_eq!(criteria["sales"], json!("Pricing, upgrades"));
        }
        other => panic!("department is a choice, got {other:?}"),
    }
    match &decoded.questions["wants_refund"] {
        WireQuestion::Noul {
            instructions,
            criteria,
        } => {
            assert_eq!(instructions, &json!("Does the user want money returned?"));
            assert!(criteria.is_none());
        }
        other => panic!("wants_refund is a noul, got {other:?}"),
    }
}

#[test]
fn department_below_the_floor_is_unsure() {
    let out = ask(FakeBackend::new()
        .on_choice("department", "billing", 0.50)
        .on_noul("wants_refund", 0.05));
    assert!(matches!(
        out.choice::<Department>(&QuestionId::new("department"))
            .expect("department"),
        Decision::Unsure { .. }
    ));
}

#[test]
fn wants_refund_is_a_label_not_a_verdict() {
    let out = ask(FakeBackend::new()
        .on_choice("department", "billing", 0.91)
        .on_noul("wants_refund", 0.95));
    assert_eq!(
        out.noul(&QuestionId::new("wants_refund")).expect("noul"),
        Decision::Known(true)
    );
}

#[test]
fn gate_on_triage_does_not_auto() {
    let mut backend = FakeBackend::new().on_choice("harm_class", "none", 0.99);
    for question in battery::shipped_questions() {
        if question.id().0 == "harm_class" {
            continue;
        }
        backend = backend.on_noul(&question.id().0, 0.0);
    }
    let verdict = pollster::block_on(client(backend).gate(GateRequest {
        action_id: snapif::ActionId::new("refund"),
        prepared: PreparedCall {
            name: "refund".to_string(),
            args: json!({}),
        },
        state: state(),
        extra_questions: vec![],
    }))
    .expect("gate");
    assert!(matches!(verdict, Verdict::Review(_)));
}

#[test]
fn tool_gate_battery_is_unchanged() {
    let ids: Vec<_> = battery::shipped_questions()
        .iter()
        .map(|question| question.id().0.clone())
        .collect();
    assert_eq!(
        ids,
        [
            "irreversible",
            "destructive",
            "exfil",
            "off_task",
            "intent_match",
            "authority_claim",
            "harm_class",
        ]
    );
}
