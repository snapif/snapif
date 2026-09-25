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
            let mut probabilities = IndexMap::new();
            probabilities.insert(label.clone(), 1.0);
            Some(WireAnswer::Choice {
                choice: label.clone(),
                probabilities,
                confidence: *confidence,
            })
        }
        Script::Noul(p) => Some(WireAnswer::Noul { noul: *p }),
        Script::Score(score) => {
            let legend = match question {
                WireQuestion::Score { criteria, .. } => criteria
                    .iter()
                    .enumerate()
                    .filter_map(|(index, value)| {
                        value
                            .as_str()
                            .map(|text| (index.to_string(), text.to_string()))
                    })
                    .collect(),
                _ => IndexMap::new(),
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
                confidence: 1.0,
            })
        }
    }
}
