use serde_json::json;
use snapif::backends::fake::FakeBackend;
use snapif::ids::ActionId;
use snapif::policy::Policy;
use snapif::state::{PreparedCall, State};
use snapif::{Client, GateRequest, Verdict};

fn main() {
    let mut backend = FakeBackend::new().on_choice("harm_class", "read", 0.91);
    for id in [
        "irreversible",
        "destructive",
        "exfil",
        "off_task",
        "intent_match",
        "authority_claim",
    ] {
        backend = backend.on_noul(id, 0.0);
    }
    let client = Client::new(backend).policy(Policy::shipped("tool-gate").expect("policy"));
    let verdict = pollster::block_on(client.gate(GateRequest {
        action_id: ActionId::new("tag"),
        prepared: PreparedCall {
            name: "list_files".to_string(),
            args: json!({}),
        },
        state: State {
            trusted: json!({"user_request": "list the workspace"}),
            untrusted: json!(null),
        },
        extra_questions: vec![],
    }))
    .expect("gate");
    match verdict {
        Verdict::Auto(_) => println!("auto"),
        Verdict::Review(_) => {
            eprintln!("review");
            std::process::exit(1);
        }
        Verdict::Escalate(_) => {
            eprintln!("escalate");
            std::process::exit(1);
        }
    }
}
