use indexmap::IndexMap;
use serde_json::{Value, json};
use snapif::question::ChoiceQ;
use snapif::{Client, FakeBackend, Policy, Question, QuestionId, State};

fn department() -> Question {
    let mut criteria = IndexMap::new();
    criteria.insert("billing".to_string(), json!("Payments, invoices, refunds"));
    Question::Choice(ChoiceQ {
        id: QuestionId::new("department"),
        instructions: json!("Which department owns this request?"),
        criteria,
    })
}

fn state() -> State {
    State {
        trusted: json!({}),
        untrusted: Value::Null,
    }
}

fn ask(client: Client<FakeBackend>) -> Client<FakeBackend> {
    pollster::block_on(client.ask(state(), vec![department()])).expect("ask");
    client
}

#[test]
fn builder_sends_the_model_id() {
    let client = ask(
        Client::new(FakeBackend::new().on_choice("department", "billing", 0.91))
            .model("  local-model  ")
            .expect("model")
            .policy(Policy::shipped("tool-gate").expect("policy")),
    );
    assert_eq!(
        client.backend().last_model().as_deref(),
        Some("local-model")
    );
}

#[test]
fn blank_builder_is_policy_error() {
    match Client::new(FakeBackend::new()).model("   ") {
        Err(err) => assert!(err.to_string().contains("SNAPIF_MODEL"), "{err}"),
        Ok(_) => panic!("blank model"),
    }
}

#[test]
fn default_model_on_the_wire_is_jev_latest() {
    let client = ask(
        Client::new(FakeBackend::new().on_choice("department", "billing", 0.91))
            .policy(Policy::shipped("tool-gate").expect("policy")),
    );
    assert_eq!(client.backend().last_model().as_deref(), Some("jev-latest"));
}
