use std::future::Future;
use std::time::{Duration, Instant};

use indexmap::IndexMap;

use crate::answer::{ChoiceAnswer, NoulAnswer, ScoreAnswer};
use crate::backends::fake::FakeBackend;
use crate::error::{BackendError, DecodeError, Error, PolicyError};
use crate::ids::QuestionId;
use crate::policy::Policy;
use crate::question::{ChoiceLabels, Question, ScoreLabels};
use crate::state::State;
use crate::usage::UsageFn;
use crate::verdict::{Decision, UnsureReason, UntypedDecision};
use crate::wire::{
    self, Usage, WireAnswer, WireQuestion, WireRequest, WireResponse, renormalize_probabilities,
};

#[derive(Debug)]
pub struct Evaluated {
    pub wire: WireResponse,
    pub meta: IndexMap<String, AnswerMeta>,
    pub backend_id: String,
}

#[derive(Debug, Clone, Default)]
pub struct AnswerMeta {
    pub original_prob_sum: Option<f64>,
    pub cascade_hop: Option<CascadeHop>,
    pub first_hop_error: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CascadeHop {
    First,
    Fallback,
}

pub trait Backend: Send + Sync {
    fn id(&self) -> &str;
    fn evaluate(
        &self,
        req: WireRequest,
        deadline: Instant,
    ) -> impl Future<Output = Result<Evaluated, BackendError>> + Send;
}

pub enum AnyBackend {
    Fake(FakeBackend),
}

impl Backend for AnyBackend {
    fn id(&self) -> &str {
        match self {
            AnyBackend::Fake(backend) => backend.id(),
        }
    }

    async fn evaluate(
        &self,
        req: WireRequest,
        deadline: Instant,
    ) -> Result<Evaluated, BackendError> {
        match self {
            AnyBackend::Fake(backend) => backend.evaluate(req, deadline).await,
        }
    }
}

pub struct Client<B: Backend> {
    backend: B,
    policy: Option<Policy>,
    on_usage: Option<UsageFn>,
    timeout: Duration,
}

impl<B: Backend> Client<B> {
    pub fn new(backend: B) -> Self {
        Self {
            backend,
            policy: None,
            on_usage: None,
            timeout: Duration::from_millis(2000),
        }
    }

    pub fn policy(mut self, policy: Policy) -> Self {
        self.policy = Some(policy);
        self
    }

    pub fn on_usage(mut self, on_usage: UsageFn) -> Self {
        self.on_usage = Some(on_usage);
        self
    }

    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn backend(&self) -> &B {
        &self.backend
    }

    /// Observation. Not an execute path.
    ///
    /// Known versus Unsure uses `choice.escalate_below` and `choice.signal`.
    /// Per-action auto thresholds are not consulted, and `prepared` is not injected.
    pub async fn ask(&self, state: State, questions: Vec<Question>) -> Result<AskOut, Error> {
        let policy = self
            .policy
            .as_ref()
            .ok_or_else(|| Error::Policy(PolicyError::Invariant("policy".to_string())))?;
        let mut request = WireRequest {
            model: "jev-latest".to_string(),
            state: state.to_wire(),
            questions: questions.iter().map(wire_question).collect(),
        };
        let encoded = wire::encode(&request)?;
        if encoded.truncated_untrusted {
            request = wire::decode_request(&encoded.body)?;
        }
        let deadline = Instant::now() + self.timeout;
        let evaluated = self
            .backend
            .evaluate(request.clone(), deadline)
            .await
            .map_err(|err| map_backend(err, self.timeout))?;
        if let Some(on_usage) = &self.on_usage {
            on_usage(evaluated.wire.usage);
        }
        wire::check_response(&request.questions, &evaluated.wire)?;
        let mut decisions = IndexMap::new();
        let mut meta = IndexMap::new();
        for (id, question) in &request.questions {
            let Some(answer) = evaluated.wire.answers.get(id) else {
                continue;
            };
            record_prob_sum(&mut meta, id, answer);
            let key = QuestionId::new(id);
            let decision = untyped(policy, question, answer).map_err(Error::Decode)?;
            decisions.insert(key, decision);
        }
        Ok(AskOut {
            decisions,
            usage: evaluated.wire.usage,
            backend_id: evaluated.backend_id,
            meta,
        })
    }
}

impl Client<AnyBackend> {
    /// `SNAPIF_BACKEND` is required. Unset is [`Error::Policy`], not a silent fake.
    pub fn from_env() -> Result<Self, Error> {
        let name = std::env::var("SNAPIF_BACKEND")
            .map_err(|_| Error::Policy(PolicyError::Invariant("SNAPIF_BACKEND".to_string())))?;
        match name.as_str() {
            "fake" => {
                let policy = Policy::shipped("tool-gate")?;
                Ok(Client::new(AnyBackend::Fake(FakeBackend::new())).policy(policy))
            }
            other => Err(Error::Policy(PolicyError::Invariant(other.to_string()))),
        }
    }
}

#[derive(Debug)]
#[non_exhaustive]
pub struct AskOut {
    pub decisions: IndexMap<QuestionId, UntypedDecision>,
    pub usage: Usage,
    pub backend_id: String,
    pub meta: IndexMap<String, AnswerMeta>,
}

impl AskOut {
    pub fn choice<T: ChoiceLabels>(&self, id: &QuestionId) -> Result<Decision<T>, DecodeError> {
        match self.decisions.get(id) {
            None => Err(DecodeError::MissingAnswer { key: id.clone() }),
            Some(UntypedDecision::Choice(Decision::Known(label))) => match T::from_label(label) {
                Some(value) => Ok(Decision::Known(value)),
                None => Err(DecodeError::UnknownLabel {
                    key: id.clone(),
                    label: label.clone(),
                }),
            },
            Some(UntypedDecision::Choice(Decision::Unsure { reason, guess })) => {
                Ok(Decision::Unsure {
                    reason: reason.clone(),
                    guess: guess.as_deref().and_then(T::from_label),
                })
            }
            Some(_) => Err(DecodeError::TypeMismatch { key: id.clone() }),
        }
    }

    pub fn score<T: ScoreLabels>(&self, id: &QuestionId) -> Result<Decision<T>, DecodeError> {
        match self.decisions.get(id) {
            None => Err(DecodeError::MissingAnswer { key: id.clone() }),
            Some(UntypedDecision::Score(Decision::Known(score))) => {
                score_label::<T>(*score, id.clone())
            }
            Some(UntypedDecision::Score(Decision::Unsure { reason, guess })) => {
                Ok(Decision::Unsure {
                    reason: reason.clone(),
                    guess: guess.and_then(|score| score_index(score).and_then(T::from_index)),
                })
            }
            Some(_) => Err(DecodeError::TypeMismatch { key: id.clone() }),
        }
    }

    pub fn noul(&self, id: &QuestionId) -> Result<Decision<bool>, DecodeError> {
        match self.decisions.get(id) {
            None => Err(DecodeError::MissingAnswer { key: id.clone() }),
            Some(UntypedDecision::Noul(decision)) => Ok(decision.clone()),
            Some(_) => Err(DecodeError::TypeMismatch { key: id.clone() }),
        }
    }
}

fn score_label<T: ScoreLabels>(score: f64, id: QuestionId) -> Result<Decision<T>, DecodeError> {
    match score_index(score).and_then(T::from_index) {
        Some(value) => Ok(Decision::Known(value)),
        None => Err(DecodeError::OutOfRange { key: id }),
    }
}

fn score_index(score: f64) -> Option<usize> {
    if score.is_finite() && score >= 0.0 {
        Some(score.round() as usize)
    } else {
        None
    }
}

fn map_backend(err: BackendError, timeout: Duration) -> Error {
    match err {
        BackendError::Timeout => Error::Timeout(timeout),
        BackendError::RateLimit => Error::RateLimit,
        BackendError::Overloaded => Error::Overloaded,
        BackendError::Auth => Error::Auth("auth".to_string()),
        other => Error::Backend(other.to_string()),
    }
}

fn wire_question(question: &Question) -> (String, WireQuestion) {
    match question {
        Question::Choice(choice) => (
            choice.id.to_string(),
            WireQuestion::Choice {
                instructions: choice.instructions.clone(),
                criteria: choice.criteria.clone(),
            },
        ),
        Question::Score(score) => (
            score.id.to_string(),
            WireQuestion::Score {
                instructions: score.instructions.clone(),
                criteria: score.criteria.clone(),
            },
        ),
        Question::Noul(noul) => (
            noul.id.to_string(),
            WireQuestion::Noul {
                instructions: noul.instructions.clone(),
                criteria: noul.criteria.clone(),
            },
        ),
    }
}

fn record_prob_sum(meta: &mut IndexMap<String, AnswerMeta>, id: &str, answer: &WireAnswer) {
    let probabilities = match answer {
        WireAnswer::Choice { probabilities, .. } | WireAnswer::Score { probabilities, .. } => {
            probabilities
        }
        WireAnswer::Noul { .. } => return,
    };
    let (_, original_sum) = renormalize_probabilities(probabilities);
    if (original_sum - 1.0).abs() > 1e-6 {
        meta.insert(
            id.to_string(),
            AnswerMeta {
                original_prob_sum: Some(original_sum),
                ..AnswerMeta::default()
            },
        );
    }
}

fn untyped(
    policy: &Policy,
    question: &WireQuestion,
    answer: &WireAnswer,
) -> Result<UntypedDecision, DecodeError> {
    match (question, answer) {
        (
            WireQuestion::Choice { .. },
            WireAnswer::Choice {
                choice,
                probabilities,
                confidence,
            },
        ) => {
            let decoded = ChoiceAnswer {
                label: choice.clone(),
                confidence: *confidence,
                probabilities: probabilities.clone(),
            };
            let signal = decoded.signal(policy.choice.signal);
            let floor = policy.choice.escalate_below;
            let decision = if signal < floor {
                Decision::Unsure {
                    reason: UnsureReason::BelowFloor {
                        confidence: signal,
                        floor,
                    },
                    guess: Some(decoded.label),
                }
            } else {
                Decision::Known(decoded.label)
            };
            Ok(UntypedDecision::Choice(decision))
        }
        (
            WireQuestion::Score { .. },
            WireAnswer::Score {
                score,
                probabilities,
                confidence,
                ..
            },
        ) => {
            let decoded = ScoreAnswer {
                score: *score,
                confidence: *confidence,
                probabilities: probabilities.clone(),
            };
            let signal = decoded.signal(policy.choice.signal);
            let floor = policy.choice.escalate_below;
            let decision = if signal < floor {
                Decision::Unsure {
                    reason: UnsureReason::BelowFloor {
                        confidence: signal,
                        floor,
                    },
                    guess: Some(decoded.score),
                }
            } else {
                Decision::Known(decoded.score)
            };
            Ok(UntypedDecision::Score(decision))
        }
        (WireQuestion::Noul { .. }, WireAnswer::Noul { noul }) => {
            let decision = NoulAnswer { p: *noul }.decide(&policy.noul, None);
            Ok(UntypedDecision::Noul(decision))
        }
        _ => Err(DecodeError::TypeMismatch {
            key: QuestionId::new("answer"),
        }),
    }
}
