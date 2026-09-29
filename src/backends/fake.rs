use std::sync::Mutex;
use std::time::{Duration, Instant};

use indexmap::IndexMap;
use serde_json::Value;

use crate::backend::{Backend, Evaluated};
use crate::error::BackendError;
use crate::wire::{Usage, WireAnswer, WireQuestion, WireRequest, WireResponse};

enum Script {
    Choice { label: String, confidence: f64 },
    Noul(f64),
    Score { score: f64, confidence: f64 },
    Timeout,
}

pub struct FakeBackend {
    scripts: IndexMap<String, Script>,
    last_state: Mutex<Option<Value>>,
    last_model: Mutex<Option<String>>,
    calls: Mutex<u64>,
    delay: Duration,
}

impl FakeBackend {
    // An empty script map is not a default backend.
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        Self {
            scripts: IndexMap::new(),
            last_state: Mutex::new(None),
            last_model: Mutex::new(None),
            calls: Mutex::new(0),
            delay: Duration::ZERO,
        }
    }

    /// Sleep this long at the start of `evaluate`, then honor the deadline.
    pub fn delay(mut self, delay: Duration) -> Self {
        self.delay = delay;
        self
    }

    pub fn on_choice(mut self, id: &str, label: &str, confidence: f64) -> Self {
        self.scripts.insert(
            id.to_string(),
            Script::Choice {
                label: label.to_string(),
                confidence,
            },
        );
        self
    }

    pub fn on_noul(mut self, id: &str, p: f64) -> Self {
        self.scripts.insert(id.to_string(), Script::Noul(p));
        self
    }

    /// Script the scale point `score`. Confidence stays 1, including for 0 and 1.
    pub fn on_score(self, id: &str, score: f64) -> Self {
        self.on_score_with_confidence(id, score, 1.0)
    }

    /// Script `score` and the confidence a `confidence` signal reads.
    ///
    /// [`Self::on_score`] uses confidence 1. Pass a lower confidence when the
    /// script should miss the floor.
    pub fn on_score_with_confidence(mut self, id: &str, score: f64, confidence: f64) -> Self {
        let confidence = if confidence.is_finite() {
            confidence
        } else {
            0.0
        };
        self.scripts
            .insert(id.to_string(), Script::Score { score, confidence });
        self
    }

    pub fn on_timeout(mut self, id: &str) -> Self {
        self.scripts.insert(id.to_string(), Script::Timeout);
        self
    }

    pub fn last_state(&self) -> Option<Value> {
        self.last_state.lock().ok().and_then(|guard| guard.clone())
    }

    pub fn last_model(&self) -> Option<String> {
        self.last_model.lock().ok().and_then(|guard| guard.clone())
    }

    pub fn calls(&self) -> u64 {
        self.calls.lock().map(|guard| *guard).unwrap_or(0)
    }
}

impl Backend for FakeBackend {
    fn id(&self) -> &str {
        "fake"
    }

    async fn evaluate(
        &self,
        req: WireRequest,
        deadline: Instant,
    ) -> Result<Evaluated, BackendError> {
        {
            let mut guard = self
                .last_state
                .lock()
                .map_err(|err| BackendError::Transport(err.to_string()))?;
            *guard = Some(req.state.clone());
        }
        if let Ok(mut guard) = self.last_model.lock() {
            *guard = Some(req.model.clone());
        }
        if let Ok(mut guard) = self.calls.lock() {
            *guard += 1;
        }
        if !self.delay.is_zero() {
            std::thread::sleep(self.delay);
        }
        if Instant::now() >= deadline {
            return Err(BackendError::Timeout);
        }
        if req
            .questions
            .keys()
            .any(|id| matches!(self.scripts.get(id), Some(Script::Timeout)))
        {
            return Err(BackendError::Timeout);
        }
        let mut answers = IndexMap::new();
        for (id, question) in &req.questions {
            let Some(script) = self.scripts.get(id) else {
                continue;
            };
            if let Some(answer) = scripted(script, question) {
                answers.insert(id.clone(), answer);
            }
        }
        Ok(Evaluated {
            wire: WireResponse {
                model: "fake".to_string(),
                answers,
                usage: Usage::default(),
            },
            meta: IndexMap::new(),
            backend_id: self.id().to_string(),
        })
    }
}

fn scripted(script: &Script, question: &WireQuestion) -> Option<WireAnswer> {
    match script {
        Script::Timeout => None,
        Script::Choice { label, confidence } => {
            let labels: Vec<String> = match question {
                WireQuestion::Choice { criteria, .. } => criteria.keys().cloned().collect(),
                _ => Vec::new(),
            };
            Some(WireAnswer::Choice {
                choice: label.clone(),
                probabilities: choice_probabilities(label, *confidence, &labels),
                confidence: *confidence,
            })
        }
        Script::Noul(p) => Some(WireAnswer::Noul { noul: *p }),
        Script::Score { score, confidence } => {
            let (legend, width) = match question {
                WireQuestion::Score { criteria, .. } => {
                    let legend = criteria
                        .iter()
                        .enumerate()
                        .filter_map(|(index, value)| {
                            value
                                .as_str()
                                .map(|text| (index.to_string(), text.to_string()))
                        })
                        .collect();
                    (legend, criteria.len())
                }
                _ => (IndexMap::new(), 0),
            };
            Some(WireAnswer::Score {
                score: *score,
                legend,
                probabilities: score_probabilities(*score, *confidence, width),
                confidence: *confidence,
            })
        }
    }
}

fn split_mass(confidence: f64, width: usize) -> (f64, f64) {
    if width <= 1 {
        return (1.0, 0.0);
    }
    let each = (1.0 - confidence) / (width - 1) as f64;
    if confidence.is_finite() && each > confidence {
        let uniform = 1.0 / width as f64;
        return (uniform, uniform);
    }
    (confidence, each)
}

/// One criterion cannot carry a lower mass: the only probability is 1.0, so
/// `top_prob` and `margin` stay 1.0. With more criteria, the chosen label
/// gets `confidence` and the others share the rest, so `top_prob` equals
/// `confidence` while no other label exceeds that mass. Otherwise every
/// label is `1/n`, so a rival cannot be the unique mode.
fn choice_probabilities(label: &str, confidence: f64, labels: &[String]) -> IndexMap<String, f64> {
    let mut probabilities = IndexMap::new();
    let others: Vec<&String> = labels.iter().filter(|key| key.as_str() != label).collect();
    if others.is_empty() {
        probabilities.insert(label.to_string(), 1.0);
        return probabilities;
    }
    let (scripted, each) = split_mass(confidence, others.len() + 1);
    probabilities.insert(label.to_string(), scripted);
    for key in others {
        probabilities.insert(key.clone(), each);
    }
    probabilities
}

fn score_probabilities(score: f64, confidence: f64, width: usize) -> IndexMap<String, f64> {
    let mut probabilities = IndexMap::new();
    if !(score.is_finite() && score >= 0.0) {
        return probabilities;
    }
    let index = score.round() as usize;
    if width >= 2 && confidence < 1.0 && index < width {
        let (scripted, each) = split_mass(confidence, width);
        for slot in 0..width {
            let mass = if slot == index { scripted } else { each };
            probabilities.insert(slot.to_string(), mass);
        }
        return probabilities;
    }
    probabilities.insert(index.to_string(), 1.0);
    probabilities
}

#[cfg(test)]
mod tests {
    use super::{choice_probabilities, score_probabilities};
    use crate::Client;
    use crate::backends::cascade::battery_ids;
    use crate::gate::GateRequest;
    use crate::ids::{ActionId, QuestionId};
    use crate::policy::Policy;
    use crate::question::{ChoiceQ, Question, ScoreQ};
    use crate::state::{PreparedCall, State};
    use crate::verdict::{Decision, UntypedDecision, Verdict};
    use indexmap::IndexMap;
    use serde_json::json;

    #[test]
    fn one_label_stays_at_one_and_a_wider_choice_keeps_confidence() {
        let only = choice_probabilities("read", 0.5, &["read".to_string()]);
        assert_eq!(only.get("read"), Some(&1.0));
        let labels = ["none", "read", "write", "exec", "network", "money"]
            .map(str::to_string)
            .to_vec();
        let spread = choice_probabilities("read", 0.5, &labels);
        assert_eq!(spread.get("read"), Some(&0.5));
        assert_eq!(spread.values().filter(|value| **value > 0.5).count(), 0);
        let known = score_probabilities(0.0, 1.0, 5);
        assert_eq!(known.get("0"), Some(&1.0));
        assert_eq!(known.len(), 1);
        let low = score_probabilities(2.0, 0.5, 5);
        assert_eq!(low.get("2"), Some(&0.5));
        assert_eq!(low.values().filter(|value| **value > 0.5).count(), 0);
        let tied = choice_probabilities("no", 0.0, &["no".into(), "yes".into()]);
        let scripted = *tied.get("no").expect("no");
        assert_eq!(tied.values().filter(|value| **value > scripted).count(), 0);
        assert_eq!(tied.get("yes").copied(), Some(scripted));
        let tied_score = score_probabilities(0.0, 0.0, 2);
        let low_bin = *tied_score.get("0").expect("0");
        let high_bin = *tied_score.get("1").expect("1");
        assert!(low_bin >= high_bin, "0={low_bin} 1={high_bin}");
        assert_eq!(low_bin, high_bin);
    }

    #[test]
    fn top_prob_policy_does_not_auto_at_confidence_half() {
        let policy = Policy::from_toml_str(
            r#"
schema_version = 1
fail = "closed"
cascade_min = 0.99
battery = "tool-gate"
[choice]
escalate_below = 0.8
review_below = 0.99
signal = "top_prob"
[default_action]
auto = 0.99
review = 0.8
when_unsure = "review_guess"
class = "read"
"#,
        )
        .expect("policy");
        let mut backend = super::FakeBackend::new().on_choice("harm_class", "read", 0.5);
        for id in battery_ids() {
            if id.0 != "harm_class" {
                backend = backend.on_noul(&id.0, 0.0);
            }
        }
        let client = Client::new(backend).policy(policy);
        let verdict = pollster::block_on(client.gate(GateRequest {
            action_id: ActionId::new("tag"),
            prepared: PreparedCall {
                name: "tag".to_string(),
                args: json!({}),
            },
            state: State {
                trusted: json!({}),
                untrusted: json!(null),
            },
            extra_questions: Vec::new(),
        }))
        .expect("gate");
        assert!(!matches!(verdict, Verdict::Auto(_)), "{verdict:?}");
    }

    #[test]
    fn top_prob_two_label_extra_at_zero_does_not_auto() {
        let policy = Policy::from_toml_str(
            r#"
schema_version = 1
fail = "closed"
cascade_min = 0.99
battery = "tool-gate"
[choice]
escalate_below = 0.8
review_below = 0.99
signal = "top_prob"
[default_action]
auto = 0.99
review = 0.8
when_unsure = "review_guess"
class = "read"
"#,
        )
        .expect("policy");
        let mut backend = super::FakeBackend::new()
            .on_choice("harm_class", "read", 1.0)
            .on_choice("pair", "no", 0.0);
        for id in battery_ids() {
            if id.0 != "harm_class" {
                backend = backend.on_noul(&id.0, 0.0);
            }
        }
        let mut criteria = IndexMap::new();
        criteria.insert("no".to_string(), json!("no"));
        criteria.insert("yes".to_string(), json!("yes"));
        let client = Client::new(backend).policy(policy);
        let verdict = pollster::block_on(client.gate(GateRequest {
            action_id: ActionId::new("tag"),
            prepared: PreparedCall {
                name: "tag".to_string(),
                args: json!({}),
            },
            state: State {
                trusted: json!({}),
                untrusted: json!(null),
            },
            extra_questions: vec![Question::Choice(ChoiceQ {
                id: QuestionId::new("pair"),
                instructions: json!("pair"),
                criteria,
            })],
        }))
        .expect("gate");
        assert!(!matches!(verdict, Verdict::Auto(_)), "{verdict:?}");
    }

    #[test]
    fn margin_two_point_score_at_zero_does_not_auto() {
        let policy = Policy::from_toml_str(
            r#"
schema_version = 1
fail = "closed"
cascade_min = 0.99
battery = "tool-gate"
[choice]
escalate_below = 0.8
review_below = 0.99
signal = "margin"
[default_action]
auto = 0.99
review = 0.8
when_unsure = "review_guess"
class = "read"
"#,
        )
        .expect("policy");
        let mut backend = super::FakeBackend::new()
            .on_choice("harm_class", "read", 1.0)
            .on_score_with_confidence("pair_score", 0.0, 0.0);
        for id in battery_ids() {
            if id.0 != "harm_class" {
                backend = backend.on_noul(&id.0, 0.0);
            }
        }
        let client = Client::new(backend).policy(policy);
        let verdict = pollster::block_on(client.gate(GateRequest {
            action_id: ActionId::new("tag"),
            prepared: PreparedCall {
                name: "tag".to_string(),
                args: json!({}),
            },
            state: State {
                trusted: json!({}),
                untrusted: json!(null),
            },
            extra_questions: vec![Question::Score(ScoreQ {
                id: QuestionId::new("pair_score"),
                instructions: json!("pair"),
                criteria: vec![json!("low"), json!("high")],
            })],
        }))
        .expect("gate");
        assert!(!matches!(verdict, Verdict::Auto(_)), "{verdict:?}");
    }

    #[test]
    fn ask_two_label_choice_at_zero_is_not_known() {
        let policy = Policy::from_toml_str(
            r#"
schema_version = 1
fail = "closed"
cascade_min = 0.99
battery = "tool-gate"
[choice]
escalate_below = 0.8
review_below = 0.99
signal = "top_prob"
[default_action]
auto = 0.99
review = 0.8
when_unsure = "review_guess"
class = "read"
"#,
        )
        .expect("policy");
        let backend = super::FakeBackend::new().on_choice("pair", "no", 0.0);
        let mut criteria = IndexMap::new();
        criteria.insert("no".to_string(), json!("no"));
        criteria.insert("yes".to_string(), json!("yes"));
        let client = Client::new(backend).policy(policy);
        let out = pollster::block_on(client.ask(
            State {
                trusted: json!({}),
                untrusted: json!(null),
            },
            vec![Question::Choice(ChoiceQ {
                id: QuestionId::new("pair"),
                instructions: json!("pair"),
                criteria,
            })],
        ))
        .expect("ask");
        let decision = out.decisions.get(&QuestionId::new("pair")).expect("pair");
        assert!(
            !matches!(
                decision,
                UntypedDecision::Choice(Decision::Known(label)) if label == "no"
            ),
            "{decision:?}"
        );
    }

    #[test]
    fn a_scripted_score_of_zero_does_not_drop_auto() {
        let policy = Policy::shipped("tool-gate").expect("tool-gate");
        let mut kept = super::FakeBackend::new()
            .on_choice("harm_class", "read", 0.95)
            .on_score("severity", 0.0);
        for id in battery_ids() {
            if id.0 != "harm_class" {
                kept = kept.on_noul(&id.0, 0.0);
            }
        }
        let auto = gate_with(&policy, kept, "tag");
        assert!(matches!(auto, Verdict::Auto(_)), "{auto:?}");

        let mut dropped = super::FakeBackend::new()
            .on_choice("harm_class", "exec", 1.0)
            .on_score_with_confidence("severity", 0.0, 0.0);
        for id in battery_ids() {
            if id.0 != "harm_class" {
                dropped = dropped.on_noul(&id.0, 0.0);
            }
        }
        let escalated = gate_with(&policy, dropped, "bash.rm");
        assert!(matches!(escalated, Verdict::Escalate(_)), "{escalated:?}");
    }

    fn gate_with(policy: &Policy, backend: super::FakeBackend, action: &str) -> Verdict {
        let client = Client::new(backend).policy(policy.clone());
        pollster::block_on(client.gate(GateRequest {
            action_id: ActionId::new(action),
            prepared: PreparedCall {
                name: action.to_string(),
                args: json!({}),
            },
            state: State {
                trusted: json!({}),
                untrusted: json!(null),
            },
            extra_questions: vec![crate::question::Question::Score(crate::question::ScoreQ {
                id: crate::ids::QuestionId::new("severity"),
                instructions: json!("how bad"),
                criteria: ["none", "low", "mid", "high", "max"]
                    .into_iter()
                    .map(|text| json!(text))
                    .collect(),
            })],
        }))
        .expect("gate")
    }
}
