#![no_main]

use indexmap::IndexMap;
use libfuzzer_sys::fuzz_target;
use snapif::gate::approval_excerpt;
use snapif::wire::{WireQuestion, check_response, decode_response};

fn questions() -> IndexMap<String, WireQuestion> {
    let mut choice = IndexMap::new();
    choice.insert("none".to_string(), serde_json::json!("ok"));
    choice.insert("exec".to_string(), serde_json::json!("bad"));
    let mut questions = IndexMap::new();
    questions.insert(
        "harm".to_string(),
        WireQuestion::Choice {
            instructions: serde_json::json!(""),
            criteria: choice,
        },
    );
    questions.insert(
        "authority_claim".to_string(),
        WireQuestion::Noul {
            instructions: serde_json::json!(""),
            criteria: None,
        },
    );
    questions.insert(
        "confidence".to_string(),
        WireQuestion::Score {
            instructions: serde_json::json!(""),
            criteria: vec![serde_json::json!("low"), serde_json::json!("high")],
        },
    );
    questions
}

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    let _ = approval_excerpt(&text);
    if let Ok(response) = decode_response(data) {
        let _ = check_response(&questions(), &response);
    }
});
