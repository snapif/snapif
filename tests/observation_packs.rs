use serde_json::{Value, json};
use snapif::policy::{HarmClass, UnsureVerdict};
use snapif::state::{PreparedCall, State};
use snapif::wire::{WireQuestion, WireRequest};
use snapif::{Client, Decision, FakeBackend, GateRequest, Policy, Question, Verdict};

fn state() -> State {
    State {
        trusted: json!({"trace": "the tool listed three files"}),
        untrusted: Value::Null,
    }
}

fn pack(name: &str, questions: Vec<Question>, again: Vec<Question>) {
    let policy = Policy::shipped(name).expect(name);
    assert_eq!(policy.fail, snapif::Fail::Closed);
    assert_eq!(policy.battery.0, name);
    let default = policy.default_action.as_ref().expect("default");
    assert!(default.auto.is_none());
    assert_eq!(default.class, HarmClass::Read);
    assert_eq!(default.when_unsure, UnsureVerdict::Escalate);
    let request = WireRequest {
        model: "jev-latest".to_string(),
        state: state().to_wire(None),
        questions: questions
            .iter()
            .map(|question| match question {
                Question::Noul(noul) => (
                    noul.id.0.clone(),
                    WireQuestion::Noul {
                        instructions: noul.instructions.clone(),
                        criteria: noul.criteria.clone(),
                    },
                ),
                Question::Choice(_) | Question::Score(_) => panic!("{name} ships only noul"),
            })
            .collect(),
    };
    let encoded = snapif::wire::encode(&request).expect("encode");
    snapif::wire::decode_request(&encoded.body).expect("decode");
    let mut backend = FakeBackend::new();
    for question in &questions {
        backend = backend.on_noul(&question.id().0, 0.5);
    }
    let out = pollster::block_on(
        Client::new(backend)
            .policy(Policy::shipped(name).expect(name))
            .ask(state(), again),
    )
    .expect("ask");
    for question in &questions {
        assert!(
            matches!(
                out.noul(question.id()).expect("noul"),
                Decision::Unsure { .. }
            ),
            "{name} {}",
            question.id().0
        );
    }
    let mut gate_backend = FakeBackend::new().on_choice("harm_class", "none", 0.99);
    for question in snapif::battery::shipped_questions() {
        if question.id().0 == "harm_class" {
            continue;
        }
        gate_backend = gate_backend.on_noul(&question.id().0, 0.0);
    }
    let verdict = pollster::block_on(
        Client::new(gate_backend)
            .policy(Policy::shipped(name).expect(name))
            .gate(GateRequest {
                action_id: snapif::ActionId::new("refund"),
                prepared: PreparedCall {
                    name: "refund".to_string(),
                    args: json!({}),
                },
                state: state(),
                extra_questions: vec![],
            }),
    )
    .expect("gate");
    assert!(
        !matches!(verdict, Verdict::Auto(_)),
        "{name} gate returned auto"
    );
}

#[test]
fn review_pack_is_observation() {
    let questions = snapif::review::questions();
    let ids: Vec<_> = questions
        .iter()
        .map(|question| question.id().0.clone())
        .collect();
    assert_eq!(ids, ["grounded", "harmful", "done"]);
    pack("review", questions, snapif::review::questions());
}

#[test]
fn screen_pack_is_observation() {
    let questions = snapif::screen::questions();
    let ids: Vec<_> = questions
        .iter()
        .map(|question| question.id().0.clone())
        .collect();
    assert_eq!(ids, ["sensitive", "needs_person"]);
    pack("screen", questions, snapif::screen::questions());
}

#[test]
fn tool_gate_ids_stay_put() {
    let ids: Vec<_> = snapif::battery::shipped_questions()
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
