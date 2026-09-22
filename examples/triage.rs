use serde_json::{Value, json};
use snapif::{Client, Decision, FakeBackend, Policy, QuestionId, State};

snapif::choice! {
    enum Department {
        Billing = "billing" => "Payments, invoices, refunds",
        Technical = "technical" => "Bugs, outages, integrations",
        Sales = "sales" => "Pricing, upgrades",
    }
}

fn main() {
    let backend = FakeBackend::new()
        .on_choice("department", "billing", 0.91)
        .on_noul("wants_refund", 0.05);
    let client = Client::new(backend).policy(Policy::shipped("triage").expect("shipped triage"));
    let out = pollster::block_on(client.ask(
        State {
            trusted: json!({"user_request": "invoice"}),
            untrusted: Value::Null,
        },
        snapif::triage::questions(),
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
