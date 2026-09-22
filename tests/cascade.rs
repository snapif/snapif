use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use indexmap::IndexMap;
use snapif::backend::{Backend, CascadeHop};
use snapif::backends::cascade::{CascadeRule, Cascaded};
use snapif::error::BackendError;
use snapif::ids::{ActionId, QuestionId};
use snapif::policy::Policy;
use snapif::question::ChoiceQ;
use snapif::state::{PreparedCall, State};
use snapif::verdict::UnsureReason;
use snapif::wire::{Usage, WireAnswer, WireQuestion, WireRequest};
use snapif::{Client, GateRequest, Question, Verdict};

struct Script {
    id: &'static str,
    answers: Vec<(String, WireAnswer)>,
    fail: Option<BackendError>,
    usage: Usage,
    seen: Arc<Mutex<Vec<Vec<String>>>>,
    deadlines: Arc<Mutex<Vec<Duration>>>,
}

impl Backend for Script {
    fn id(&self) -> &str {
        self.id
    }

    async fn evaluate(
        &self,
        req: WireRequest,
        deadline: Instant,
    ) -> Result<snapif::backend::Evaluated, BackendError> {
        self.seen
            .lock()
            .expect("seen")
            .push(req.questions.keys().cloned().collect());
        self.deadlines
            .lock()
            .expect("deadlines")
            .push(deadline.saturating_duration_since(Instant::now()));
        if let Some(err) = self.fail.clone() {
            return Err(err);
        }
        let mut answers = IndexMap::new();
        for (id, answer) in &self.answers {
            if req.questions.contains_key(id) {
                answers.insert(id.clone(), answer.clone());
            }
        }
        Ok(snapif::backend::Evaluated {
            wire: snapif::wire::WireResponse {
                model: self.id.to_string(),
                answers,
                usage: self.usage,
            },
            meta: IndexMap::new(),
            backend_id: self.id.to_string(),
        })
    }
}

fn choice(confidence: f64) -> WireAnswer {
    let mut probabilities = IndexMap::new();
    probabilities.insert("billing".to_string(), 1.0);
    WireAnswer::Choice {
        choice: "billing".to_string(),
        probabilities,
        confidence,
    }
}

fn noul(p: f64) -> WireAnswer {
    WireAnswer::Noul { noul: p }
}

fn request(ids: &[&str]) -> WireRequest {
    let mut questions = IndexMap::new();
    for id in ids {
        questions.insert(
            (*id).to_string(),
            WireQuestion::Noul {
                instructions: serde_json::json!("q"),
                criteria: None,
            },
        );
    }
    WireRequest {
        model: "jev-latest".to_string(),
        state: serde_json::json!({}),
        questions,
    }
}

fn rule_empty() -> CascadeRule {
    CascadeRule {
        min: 0.8,
        first_timeout: Duration::from_secs(2),
        always_fallback: Vec::new(),
    }
}

struct Pair {
    cascade: Cascaded<Script, Script>,
    fallback_seen: Arc<Mutex<Vec<Vec<String>>>>,
}

fn pair(
    first_answers: Vec<(String, WireAnswer)>,
    first_fail: Option<BackendError>,
    fallback_answers: Vec<(String, WireAnswer)>,
    fallback_fail: Option<BackendError>,
    rule: CascadeRule,
) -> Pair {
    let fallback_seen = Arc::new(Mutex::new(Vec::new()));
    let cascade = Cascaded::new(
        Script {
            id: "first",
            answers: first_answers,
            fail: first_fail,
            usage: Usage {
                input_tokens: 1,
                output_tokens: 1,
            },
            seen: Arc::new(Mutex::new(Vec::new())),
            deadlines: Arc::new(Mutex::new(Vec::new())),
        },
        Script {
            id: "fallback",
            answers: fallback_answers,
            fail: fallback_fail,
            usage: Usage {
                input_tokens: 2,
                output_tokens: 3,
            },
            seen: Arc::clone(&fallback_seen),
            deadlines: Arc::new(Mutex::new(Vec::new())),
        },
        rule,
    );
    Pair {
        cascade,
        fallback_seen,
    }
}

fn run(pair: &Pair, ids: &[&str]) -> Result<snapif::backend::Evaluated, BackendError> {
    pollster::block_on(
        pair.cascade
            .evaluate(request(ids), Instant::now() + Duration::from_secs(2)),
    )
}

#[test]
fn high_confidence_skips_fallback() {
    let pair = pair(
        vec![("harm_class".into(), choice(0.95))],
        None,
        vec![("harm_class".into(), choice(0.1))],
        None,
        rule_empty(),
    );
    let evaluated = run(&pair, &["harm_class"]).expect("ok");
    assert!(pair.fallback_seen.lock().expect("seen").is_empty());
    assert_eq!(evaluated.wire.usage.input_tokens, 1);
    assert_eq!(
        evaluated.meta["harm_class"].cascade_hop,
        Some(CascadeHop::First)
    );
}

#[test]
fn below_min_retries_only_that_id() {
    let pair = pair(
        vec![("keep".into(), choice(0.95)), ("retry".into(), choice(0.4))],
        None,
        vec![("retry".into(), choice(0.99))],
        None,
        rule_empty(),
    );
    let evaluated = run(&pair, &["keep", "retry"]).expect("ok");
    let seen = pair.fallback_seen.lock().expect("seen");
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0], vec!["retry".to_string()]);
    match &evaluated.wire.answers["retry"] {
        WireAnswer::Choice { confidence, .. } => assert!((*confidence - 0.99).abs() < 1e-9),
        other => panic!("unexpected {other:?}"),
    }
    assert_eq!(evaluated.wire.usage.input_tokens, 3);
    assert_eq!(evaluated.wire.usage.output_tokens, 4);
    assert_eq!(evaluated.wire.model, "fallback");
    assert_eq!(evaluated.backend_id, "cascade:first+fallback");
    assert_eq!(evaluated.meta["keep"].cascade_hop, Some(CascadeHop::First));
    assert_eq!(
        evaluated.meta["retry"].cascade_hop,
        Some(CascadeHop::Fallback)
    );
}

#[test]
fn ask_keeps_fallback_hop_when_prob_sum_is_overlaid() {
    // 0.25 + 0.25 is off 1, so ask must overlay original_prob_sum on the hop row.
    let mut probabilities = IndexMap::new();
    probabilities.insert("billing".to_string(), 0.25);
    probabilities.insert("sales".to_string(), 0.25);
    let low = WireAnswer::Choice {
        choice: "billing".to_string(),
        probabilities: probabilities.clone(),
        confidence: 0.4,
    };
    let high = WireAnswer::Choice {
        choice: "billing".to_string(),
        probabilities,
        confidence: 0.99,
    };
    let pair = pair(
        vec![("retry".into(), low)],
        None,
        vec![("retry".into(), high)],
        None,
        rule_empty(),
    );
    let mut criteria = IndexMap::new();
    criteria.insert("billing".to_string(), serde_json::json!("Payments"));
    let client = Client::new(pair.cascade).policy(Policy::shipped("tool-gate").expect("policy"));
    let out = pollster::block_on(client.ask(
        State {
            trusted: serde_json::json!({}),
            untrusted: serde_json::json!(null),
        },
        vec![Question::Choice(ChoiceQ {
            id: QuestionId::new("retry"),
            instructions: serde_json::json!("Which department?"),
            criteria,
        })],
    ))
    .expect("ask");
    let row = out.meta.get("retry").expect("meta");
    assert_eq!(row.cascade_hop, Some(CascadeHop::Fallback));
    let sum = row.original_prob_sum.expect("original_prob_sum");
    assert!((sum - 0.5).abs() < 1e-9);
    assert!(!row.first_hop_error);
}

#[test]
fn first_timeout_sends_the_full_request() {
    let pair = pair(
        vec![("harm_class".into(), choice(0.99))],
        Some(BackendError::Timeout),
        vec![("harm_class".into(), choice(0.91))],
        None,
        rule_empty(),
    );
    let evaluated = run(&pair, &["harm_class"]).expect("fallback");
    let seen = pair.fallback_seen.lock().expect("seen");
    assert_eq!(seen[0], vec!["harm_class".to_string()]);
    assert!(evaluated.meta["harm_class"].first_hop_error);
    assert_eq!(
        evaluated.meta["harm_class"].cascade_hop,
        Some(CascadeHop::Fallback)
    );
}

#[test]
fn second_hop_error_keeps_the_first_answers() {
    let pair = pair(
        vec![("retry".into(), choice(0.2))],
        None,
        vec![],
        Some(BackendError::Overloaded),
        rule_empty(),
    );
    let evaluated = run(&pair, &["retry"]).expect("kept");
    match &evaluated.wire.answers["retry"] {
        WireAnswer::Choice { confidence, .. } => assert!((*confidence - 0.2).abs() < 1e-9),
        other => panic!("unexpected {other:?}"),
    }
    assert_eq!(evaluated.wire.usage.input_tokens, 1);
}

#[test]
fn battery_id_is_retried_even_when_pseudo_confidence_is_high() {
    let mut rule = rule_empty();
    rule.always_fallback = vec![QuestionId::new("destructive")];
    let pair = pair(
        vec![("destructive".into(), noul(0.99))],
        None,
        vec![("destructive".into(), noul(0.1))],
        None,
        rule,
    );
    let evaluated = run(&pair, &["destructive"]).expect("ok");
    assert_eq!(pair.fallback_seen.lock().expect("seen").len(), 1);
    match &evaluated.wire.answers["destructive"] {
        WireAnswer::Noul { noul } => assert!((*noul - 0.1).abs() < 1e-9),
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn first_hop_budget_is_capped_by_first_timeout() {
    let deadlines = Arc::new(Mutex::new(Vec::new()));
    let cascade = Cascaded::new(
        Script {
            id: "first",
            answers: vec![("harm_class".into(), choice(0.95))],
            fail: None,
            usage: Usage::default(),
            seen: Arc::new(Mutex::new(Vec::new())),
            deadlines: Arc::clone(&deadlines),
        },
        Script {
            id: "fallback",
            answers: vec![],
            fail: None,
            usage: Usage::default(),
            seen: Arc::new(Mutex::new(Vec::new())),
            deadlines: Arc::new(Mutex::new(Vec::new())),
        },
        CascadeRule {
            min: 0.8,
            first_timeout: Duration::from_millis(400),
            always_fallback: Vec::new(),
        },
    );
    pollster::block_on(cascade.evaluate(
        request(&["harm_class"]),
        Instant::now() + Duration::from_secs(2),
    ))
    .expect("ok");
    let recorded = deadlines.lock().expect("deadlines")[0];
    assert!(recorded <= Duration::from_millis(450), "{recorded:?}");
}

#[test]
fn shadow_override_does_not_rewrite_auto() {
    let mut backend =
        snapif::backends::fake::FakeBackend::new().on_choice("harm_class", "read", 0.91);
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
    let client = Client::new(backend)
        .policy(Policy::shipped("tool-gate").expect("policy"))
        .shadow(true);
    let verdict = pollster::block_on(client.gate(GateRequest {
        action_id: ActionId::new("tag"),
        prepared: PreparedCall {
            name: "tag".into(),
            args: serde_json::json!({}),
        },
        state: State {
            trusted: serde_json::json!({}),
            untrusted: serde_json::json!(null),
        },
        extra_questions: vec![],
    }))
    .expect("gate");
    match verdict {
        Verdict::Auto(hint) => {
            assert!(hint.shadow);
            assert_eq!(hint.action_id.0, "tag");
        }
        other => panic!("rewritten {other:?}"),
    }
}

fn labeled_choice(label: &str, confidence: f64) -> WireAnswer {
    let mut probabilities = IndexMap::new();
    probabilities.insert(label.to_string(), 1.0);
    WireAnswer::Choice {
        choice: label.to_string(),
        probabilities,
        confidence,
    }
}

fn battery_rows(harm: f64) -> Vec<(String, WireAnswer)> {
    let mut rows = vec![("harm_class".to_string(), labeled_choice("read", harm))];
    for id in [
        "irreversible",
        "destructive",
        "exfil",
        "off_task",
        "intent_match",
        "authority_claim",
    ] {
        rows.push((id.to_string(), noul(0.0)));
    }
    rows
}

#[test]
fn retried_id_still_below_min_sets_cascade_still_unsure() {
    let mut rule = CascadeRule::new(0.8);
    rule.first_timeout = Duration::from_secs(2);
    rule.always_fallback.retain(|id| id.0 == "harm_class");
    let pair = pair(battery_rows(0.99), None, battery_rows(0.4), None, rule);
    let client = Client::new(pair.cascade).policy(Policy::shipped("tool-gate").expect("policy"));
    let verdict = pollster::block_on(client.gate(GateRequest {
        action_id: ActionId::new("tag"),
        prepared: PreparedCall {
            name: "tag".into(),
            args: serde_json::json!({}),
        },
        state: State {
            trusted: serde_json::json!({}),
            untrusted: serde_json::json!(null),
        },
        extra_questions: vec![],
    }))
    .expect("gate");
    let hint = match verdict {
        Verdict::Review(hint) | Verdict::Escalate(hint) => hint,
        Verdict::Auto(hint) => panic!("auto {hint:?}"),
    };
    assert_eq!(hint.action_id.0, "tag");
    assert!(
        hint.reasons
            .iter()
            .any(|reason| matches!(reason, UnsureReason::CascadeStillUnsure))
    );
}

#[test]
fn low_cascade_min_fails_the_policy_invariant() {
    let raw = r#"
schema_version = 1
fail = "closed"
shadow = false
cascade_min = 0.5
battery = "tool-gate"
[choice]
escalate_below = 0.8
review_below = 1.0
signal = "confidence"
[noul]
yes_auto = 0.90
no_auto = 0.10
[default_action]
review = 0.8
when_unsure = "review_guess"
class = "read"
"#;
    let err = Policy::from_toml_str(raw).expect_err("floor");
    assert!(matches!(err, snapif::error::PolicyError::Invariant(_)));
}

#[test]
fn choice_confidence_at_min_is_kept() {
    let pair = pair(
        vec![("department".into(), choice(0.8))],
        None,
        vec![("department".into(), choice(0.1))],
        None,
        rule_empty(),
    );
    let evaluated = run(&pair, &["department"]).expect("ok");
    assert!(pair.fallback_seen.lock().expect("seen").is_empty());
    match &evaluated.wire.answers["department"] {
        WireAnswer::Choice { confidence, .. } => assert!((*confidence - 0.8).abs() < 1e-9),
        other => panic!("unexpected {other:?}"),
    }
    assert_eq!(
        evaluated.meta["department"].cascade_hop,
        Some(CascadeHop::First)
    );
}

#[test]
fn noul_margin_at_min_is_kept() {
    let pair = pair(
        vec![("margin".into(), noul(0.9))],
        None,
        vec![("margin".into(), noul(0.1))],
        None,
        rule_empty(),
    );
    let evaluated = run(&pair, &["margin"]).expect("ok");
    assert!(pair.fallback_seen.lock().expect("seen").is_empty());
    match &evaluated.wire.answers["margin"] {
        WireAnswer::Noul { noul } => assert!((*noul - 0.9).abs() < 1e-9),
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn confidence_just_below_min_retries_only_that_id() {
    let pair = pair(
        vec![
            ("steady".into(), choice(0.8)),
            ("edge".into(), choice(0.79)),
        ],
        None,
        vec![("edge".into(), choice(0.99))],
        None,
        rule_empty(),
    );
    let evaluated = run(&pair, &["steady", "edge"]).expect("ok");
    let seen = pair.fallback_seen.lock().expect("seen");
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0], vec!["edge".to_string()]);
    match &evaluated.wire.answers["steady"] {
        WireAnswer::Choice { confidence, .. } => assert!((*confidence - 0.8).abs() < 1e-9),
        other => panic!("unexpected {other:?}"),
    }
    match &evaluated.wire.answers["edge"] {
        WireAnswer::Choice { confidence, .. } => assert!((*confidence - 0.99).abs() < 1e-9),
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn omitted_first_answer_is_the_only_fallback_post() {
    let pair = pair(
        vec![("shown".into(), choice(0.95))],
        None,
        vec![("omitted".into(), choice(0.91))],
        None,
        rule_empty(),
    );
    let evaluated = run(&pair, &["shown", "omitted"]).expect("ok");
    let seen = pair.fallback_seen.lock().expect("seen");
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0], vec!["omitted".to_string()]);
    match &evaluated.wire.answers["shown"] {
        WireAnswer::Choice { confidence, .. } => assert!((*confidence - 0.95).abs() < 1e-9),
        other => panic!("unexpected {other:?}"),
    }
    match &evaluated.wire.answers["omitted"] {
        WireAnswer::Choice { confidence, .. } => assert!((*confidence - 0.91).abs() < 1e-9),
        other => panic!("unexpected {other:?}"),
    }
}
