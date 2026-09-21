use indexmap::IndexMap;
use serde_json::{Value, json};
use snapif::error::{BackendError, WireError};
use snapif::ids::QuestionId;
use snapif::wire::{
    ENCODE_CAP, WireAnswer, WireQuestion, WireRequest, check_response, decode_request,
    decode_response, encode, renormalize_probabilities,
};

const DEPARTMENT: &str = include_str!("conformance/department_choice.json");
const FRUSTRATION: &str = include_str!("conformance/frustration_score.json");
const NOUL_BARE: &str = include_str!("conformance/noul_no_criteria.json");
const NOUL_CRITERIA: &str = include_str!("conformance/noul_with_criteria.json");

const QUICKSTART_RESPONSE: &str = r#"{"model":"jev-1.13.0","answers":{"department":{"type":"choice","choice":"technical","confidence":0.78,"probabilities":{"technical":0.85,"sales":0.0,"billing":0.15}},"frustration":{"type":"score","score":1.0,"confidence":1.0,"legend":{"0":"Calm, just stating facts","1":"Frustrated but civil","2":"Very angry, strong language"},"probabilities":{"0":0.0,"1":1.0,"2":0.0}},"is_urgent":{"type":"noul","noul":1.0}},"usage":{"input_tokens":392,"output_tokens":65}}"#;

fn round_trip(raw: &str) -> WireRequest {
    let decoded = decode_request(raw.as_bytes()).expect("fixture decodes");
    let encoded = encode(&decoded).expect("fixture encodes");
    assert!(!encoded.truncated_untrusted);
    decode_request(&encoded.body).expect("encode round trip")
}

#[test]
fn fixtures_round_trip() {
    let department = round_trip(DEPARTMENT);
    assert!(matches!(
        department.questions.get("department"),
        Some(WireQuestion::Choice { criteria, .. }) if criteria.len() == 3
    ));
    let frustration = round_trip(FRUSTRATION);
    assert!(matches!(
        frustration.questions.get("frustration"),
        Some(WireQuestion::Score { criteria, .. }) if criteria.len() == 3
    ));

    let bare = decode_request(NOUL_BARE.as_bytes()).unwrap();
    let bare_body = encode(&bare).unwrap().body;
    let bare_json: Value = serde_json::from_slice(&bare_body).unwrap();
    assert!(
        bare_json["questions"]["is_urgent"]
            .get("criteria")
            .is_none()
    );

    let with = decode_request(NOUL_CRITERIA.as_bytes()).unwrap();
    let with_body = encode(&with).unwrap().body;
    let with_json: Value = serde_json::from_slice(&with_body).unwrap();
    assert_eq!(
        with_json["questions"]["is_urgent"]["criteria"]["true"],
        "Explicitly time-sensitive"
    );
    assert_eq!(
        with_json["questions"]["is_urgent"]["criteria"]["false"],
        "No urgency expressed"
    );
}

#[test]
fn quickstart_response_checks() {
    let mut questions = IndexMap::new();
    for raw in [DEPARTMENT, FRUSTRATION, NOUL_BARE] {
        let request = decode_request(raw.as_bytes()).unwrap();
        questions.extend(request.questions);
    }
    let response = decode_response(QUICKSTART_RESPONSE.as_bytes()).unwrap();
    check_response(&questions, &response).unwrap();
    assert!(matches!(
        response.answers.get("is_urgent"),
        Some(WireAnswer::Noul { noul: value }) if *value == 1.0
    ));
}

#[test]
fn boolean_type_is_unknown() {
    let raw = r#"{"state":"x","model":"jev-latest","questions":{"flag":{"type":"boolean","instructions":"yes?"}}}"#;
    match decode_request(raw.as_bytes()) {
        Err(WireError::UnknownType(kind)) => assert_eq!(kind, "boolean"),
        other => panic!("expected UnknownType, got {other:?}"),
    }
}

#[test]
fn noul_ignores_extra_confidence() {
    let raw = r#"{"model":"jev-1","answers":{"is_urgent":{"type":"noul","noul":0.2,"confidence":0.9}},"usage":{"input_tokens":1,"output_tokens":1}}"#;
    let response = decode_response(raw.as_bytes()).unwrap();
    assert!(matches!(
        response.answers.get("is_urgent"),
        Some(WireAnswer::Noul { noul }) if (*noul - 0.2).abs() < 1e-9
    ));
}

#[test]
fn check_response_ranges() {
    let request = decode_request(DEPARTMENT.as_bytes()).unwrap();
    let mut response = decode_response(QUICKSTART_RESPONSE.as_bytes()).unwrap();
    response.answers.shift_remove("department");
    assert!(matches!(
        check_response(&request.questions, &response),
        Err(snapif::error::DecodeError::MissingAnswer { key }) if key == QuestionId::new("department")
    ));

    let response = decode_response(
        br#"{"model":"m","answers":{"department":{"type":"choice","choice":"nope","probabilities":{},"confidence":0.1}},"usage":{"input_tokens":0,"output_tokens":0}}"#,
    )
    .unwrap();
    assert!(matches!(
        check_response(&request.questions, &response),
        Err(snapif::error::DecodeError::UnknownLabel { label, .. }) if label == "nope"
    ));

    let noul = decode_request(NOUL_BARE.as_bytes()).unwrap();
    for (raw, ok) in [(0.0, true), (1.0, true), (-0.1, false), (1.1, false)] {
        let response = decode_response(
            format!(
                r#"{{"model":"m","answers":{{"is_urgent":{{"type":"noul","noul":{raw}}}}},"usage":{{"input_tokens":0,"output_tokens":0}}}}"#
            )
            .as_bytes(),
        )
        .unwrap();
        assert_eq!(check_response(&noul.questions, &response).is_ok(), ok);
    }

    let score = decode_request(FRUSTRATION.as_bytes()).unwrap();
    for (raw, ok) in [
        (0.0, true),
        (2.0, true),
        (-1e-5, false),
        (2.0 + 1e-5, false),
    ] {
        let response = decode_response(
            format!(
                r#"{{"model":"m","answers":{{"frustration":{{"type":"score","score":{raw},"legend":{{}},"probabilities":{{}},"confidence":1.0}}}},"usage":{{"input_tokens":0,"output_tokens":0}}}}"#
            )
            .as_bytes(),
        )
        .unwrap();
        assert_eq!(check_response(&score.questions, &response).is_ok(), ok);
    }
}

#[test]
fn renormalize_returns_original_sum() {
    let mut probabilities = IndexMap::new();
    probabilities.insert("a".to_string(), 0.2);
    probabilities.insert("b".to_string(), 0.2);
    let (scaled, original_sum) = renormalize_probabilities(&probabilities);
    assert!((original_sum - 0.4).abs() < 1e-12);
    let sum: f64 = scaled.values().sum();
    assert!((sum - 1.0).abs() < 1e-12);
}

#[test]
fn encode_cap_replaces_only_untrusted() {
    let mut questions = IndexMap::new();
    questions.insert(
        "is_urgent".to_string(),
        WireQuestion::Noul {
            instructions: json!("urgent?"),
            criteria: None,
        },
    );
    let request = WireRequest {
        model: "jev-latest".to_string(),
        state: json!({
            "trusted": {"user_request": "keep"},
            "prepared": {"name": "bash", "args": {"command": "rm"}},
            "untrusted": "u".repeat(ENCODE_CAP)
        }),
        questions,
    };
    let encoded = encode(&request).unwrap();
    assert!(encoded.truncated_untrusted);
    let parsed: Value = serde_json::from_slice(&encoded.body).unwrap();
    assert_eq!(parsed["state"]["untrusted"], json!({"truncated": true}));
    assert_eq!(parsed["state"]["trusted"]["user_request"], "keep");
    assert_eq!(parsed["state"]["prepared"]["name"], "bash");

    let still = WireRequest {
        state: json!({
            "trusted": "t".repeat(ENCODE_CAP),
            "prepared": {"name": "bash"},
            "untrusted": "u".repeat(ENCODE_CAP)
        }),
        ..request.clone()
    };
    assert!(matches!(encode(&still), Err(WireError::BodyCap(_))));

    let text = WireRequest {
        state: Value::String("s".repeat(ENCODE_CAP)),
        ..request
    };
    assert!(matches!(encode(&text), Err(WireError::BodyCap(_))));
}

#[test]
fn criteria_bounds_and_rejected_display() {
    let mut wide = IndexMap::new();
    for i in 0..256 {
        wide.insert(format!("o{i}"), json!("x"));
    }
    let mut questions = IndexMap::new();
    questions.insert(
        "department".to_string(),
        WireQuestion::Choice {
            instructions: json!("which"),
            criteria: wide,
        },
    );
    let request = WireRequest {
        model: "jev-latest".to_string(),
        state: json!("x"),
        questions,
    };
    assert!(matches!(encode(&request), Err(WireError::ChoiceTooWide)));

    let empty = WireRequest {
        questions: IndexMap::from([(
            "department".to_string(),
            WireQuestion::Choice {
                instructions: json!("which"),
                criteria: IndexMap::new(),
            },
        )]),
        ..request.clone()
    };
    assert!(matches!(encode(&empty), Err(WireError::EmptyChoice)));

    let short = WireRequest {
        questions: IndexMap::from([(
            "frustration".to_string(),
            WireQuestion::Score {
                instructions: json!("mood"),
                criteria: vec![json!("only")],
            },
        )]),
        ..request
    };
    assert!(matches!(encode(&short), Err(WireError::ScoreLen(1))));

    let shown = BackendError::Rejected {
        status: 422,
        body: "no".to_string(),
    }
    .to_string();
    assert!(shown.contains("422"));
    let missing = BackendError::Rejected {
        status: 404,
        body: "gone".to_string(),
    }
    .to_string();
    assert!(missing.contains("404"));
}
