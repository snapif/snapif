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
    Score(f64),
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

    pub fn on_score(mut self, id: &str, score: f64) -> Self {
        self.scripts.insert(id.to_string(), Script::Score(score));
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
        Script::Score(score) => {
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
            let mut probabilities = IndexMap::new();
            if score.is_finite() && *score >= 0.0 {
                let index = score.round() as usize;
                probabilities.insert(index.to_string(), 1.0);
            }
            Some(WireAnswer::Score {
                score: *score,
                legend,
                probabilities,
                confidence: score_confidence(*score, width),
            })
        }
    }
}

/// One criterion cannot carry a lower mass: the only probability is 1.0, so
/// `top_prob` and `margin` stay 1.0. With more criteria, the chosen label
/// gets `confidence` and the others share the rest, so `top_prob` equals
/// `confidence` when that mass is strictly the largest.
fn choice_probabilities(label: &str, confidence: f64, labels: &[String]) -> IndexMap<String, f64> {
    let mut probabilities = IndexMap::new();
    let others: Vec<&String> = labels.iter().filter(|key| key.as_str() != label).collect();
    if others.is_empty() {
        probabilities.insert(label.to_string(), 1.0);
        return probabilities;
    }
    let each = (1.0 - confidence) / others.len() as f64;
    probabilities.insert(label.to_string(), confidence);
    for key in others {
        probabilities.insert(key.clone(), each);
    }
    probabilities
}

fn score_confidence(score: f64, width: usize) -> f64 {
    if !score.is_finite() {
        return 0.0;
    }
    if (0.0..=1.0).contains(&score) {
        return score;
    }
    let max = (width.saturating_sub(1) as f64).max(1.0);
    (score / max).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::{choice_probabilities, score_confidence};
    use crate::Client;
    use crate::backends::cascade::battery_ids;
    use crate::gate::GateRequest;
    use crate::ids::ActionId;
    use crate::policy::Policy;
    use crate::state::{PreparedCall, State};
    use crate::verdict::Verdict;
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
        assert_eq!(score_confidence(0.25, 10), 0.25);
        assert_eq!(score_confidence(1.0, 2), 1.0);
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
}
