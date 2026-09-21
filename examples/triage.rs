use std::pin::pin;
use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

use serde_json::{Value, json};
use snapif::question::{ChoiceLabels, ChoiceQ};
use snapif::{Client, Decision, FakeBackend, Policy, Question, QuestionId, State};

snapif::choice! {
    enum Department {
        Billing = "billing" => "Payments, invoices, refunds",
        Technical = "technical" => "Bugs, outages, integrations",
        Sales = "sales" => "Pricing, upgrades",
    }
}

fn main() {
    let backend = FakeBackend::new().on_choice("department", "billing", 0.91);
    let client = Client::new(backend).policy(shipped());
    let out = block_on(client.ask(
        State {
            trusted: json!({"user_request": "invoice"}),
            untrusted: Value::Null,
        },
        vec![department_question()],
    ))
    .expect("ask");
    let decision = out
        .choice::<Department>(&QuestionId::new("department"))
        .expect("choice");
    match decision {
        Decision::Known(Department::Billing) => {
            println!("billing");
        }
        Decision::Known(Department::Technical) => {
            eprintln!("technical");
            std::process::exit(1);
        }
        Decision::Known(Department::Sales) => {
            eprintln!("sales");
            std::process::exit(1);
        }
        Decision::Unsure { .. } => {
            eprintln!("unsure");
            std::process::exit(1);
        }
    }
}

fn department_question() -> Question {
    let mut criteria = indexmap::IndexMap::new();
    for (id, text) in Department::labels() {
        criteria.insert((*id).to_string(), Value::String((*text).to_string()));
    }
    Question::Choice(ChoiceQ {
        id: QuestionId::new("department"),
        instructions: json!("Which department owns this request?"),
        criteria,
    })
}

fn shipped() -> Policy {
    Policy::shipped("tool-gate").expect("shipped tool-gate")
}

fn block_on<F: std::future::Future>(future: F) -> F::Output {
    let mut future = pin!(future);
    let waker = noop_waker();
    let mut context = Context::from_waker(&waker);
    match future.as_mut().poll(&mut context) {
        Poll::Ready(value) => value,
        Poll::Pending => panic!("observation future was not ready"),
    }
}

fn noop_waker() -> Waker {
    fn clone(data: *const ()) -> RawWaker {
        RawWaker::new(data, vtable())
    }
    fn wake(_: *const ()) {}
    fn vtable() -> &'static RawWakerVTable {
        &RawWakerVTable::new(clone, wake, wake, wake)
    }
    // SAFETY: the vtable never reads the data pointer.
    unsafe { Waker::from_raw(RawWaker::new(std::ptr::null(), vtable())) }
}
