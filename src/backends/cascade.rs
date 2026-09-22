use std::time::{Duration, Instant};

use indexmap::IndexMap;

use crate::backend::{AnswerMeta, Backend, CascadeHop, Evaluated};
use crate::error::BackendError;
use crate::ids::QuestionId;
use crate::wire::{Usage, WireAnswer, WireRequest, WireResponse};

pub struct CascadeRule {
    pub min: f64,
    pub first_timeout: Duration,
    pub always_fallback: Vec<QuestionId>,
}

impl CascadeRule {
    pub fn new(min: f64) -> Self {
        Self {
            min,
            first_timeout: Duration::from_millis(400),
            always_fallback: battery_ids(),
        }
    }
}

pub struct Cascaded<A, B> {
    pub first: A,
    pub fallback: B,
    pub rule: CascadeRule,
}

impl<A: Backend, B: Backend> Cascaded<A, B> {
    pub fn new(first: A, fallback: B, rule: CascadeRule) -> Self {
        Self {
            first,
            fallback,
            rule,
        }
    }
}

pub fn battery_ids() -> Vec<QuestionId> {
    crate::battery::shipped_questions()
        .into_iter()
        .map(|question| question.id().clone())
        .collect()
}

pub(crate) fn fallback_still_below(
    meta: &IndexMap<String, AnswerMeta>,
    answers: &IndexMap<String, WireAnswer>,
    min: f64,
) -> bool {
    meta.iter().any(|(id, row)| {
        row.cascade_hop == Some(CascadeHop::Fallback)
            && answers
                .get(id)
                .is_some_and(|answer| !answer_kept(answer, min))
    })
}

pub(crate) fn answer_kept(answer: &WireAnswer, min: f64) -> bool {
    let confidence = match answer {
        WireAnswer::Choice { confidence, .. } | WireAnswer::Score { confidence, .. } => *confidence,
        WireAnswer::Noul { noul } => (2.0 * noul - 1.0).abs(),
    };
    confidence >= min
}

impl<A: Backend, B: Backend> Backend for Cascaded<A, B> {
    fn id(&self) -> &str {
        "cascade"
    }

    async fn evaluate(
        &self,
        req: WireRequest,
        deadline: Instant,
    ) -> Result<Evaluated, BackendError> {
        if Instant::now() >= deadline {
            return Err(BackendError::Timeout);
        }
        let first_budget = deadline
            .saturating_duration_since(Instant::now())
            .min(self.rule.first_timeout);
        let first_deadline = Instant::now() + first_budget;
        let first_id = self.first.id().to_string();
        let first = match self.first.evaluate(req.clone(), first_deadline).await {
            Ok(first) => first,
            Err(_) => {
                let fallback = self.fallback.evaluate(req.clone(), deadline).await?;
                return Ok(mark_fallback(fallback, &first_id, true));
            }
        };
        let retry: Vec<String> = req
            .questions
            .keys()
            .filter(|id| !keep_id(id, &first.wire, &self.rule))
            .cloned()
            .collect();
        if retry.is_empty() {
            return Ok(mark_first(first));
        }
        let mut subset = req.clone();
        subset
            .questions
            .retain(|id, _| retry.iter().any(|retry_id| retry_id == id));
        let fallback = match self.fallback.evaluate(subset, deadline).await {
            Ok(fallback) => fallback,
            Err(_) => return Ok(mark_first(first)),
        };
        Ok(merge(first, fallback, &retry))
    }
}

fn keep_id(id: &str, wire: &WireResponse, rule: &CascadeRule) -> bool {
    if rule.always_fallback.iter().any(|forced| forced.0 == id) {
        return false;
    }
    match wire.answers.get(id) {
        Some(answer) => answer_kept(answer, rule.min),
        None => false,
    }
}

fn mark_first(mut evaluated: Evaluated) -> Evaluated {
    for id in evaluated.wire.answers.keys() {
        evaluated.meta.insert(
            id.clone(),
            AnswerMeta {
                cascade_hop: Some(CascadeHop::First),
                ..AnswerMeta::default()
            },
        );
    }
    evaluated
}

fn mark_fallback(mut evaluated: Evaluated, first_id: &str, first_hop_error: bool) -> Evaluated {
    evaluated.backend_id = format!("cascade:{first_id}+{}", evaluated.backend_id);
    for id in evaluated.wire.answers.keys() {
        evaluated.meta.insert(
            id.clone(),
            AnswerMeta {
                cascade_hop: Some(CascadeHop::Fallback),
                first_hop_error,
                ..AnswerMeta::default()
            },
        );
    }
    evaluated
}

fn merge(first: Evaluated, fallback: Evaluated, retry: &[String]) -> Evaluated {
    let mut wire = first.wire;
    for id in retry {
        if let Some(answer) = fallback.wire.answers.get(id) {
            wire.answers.insert(id.clone(), answer.clone());
        }
    }
    wire.usage = Usage {
        input_tokens: wire
            .usage
            .input_tokens
            .saturating_add(fallback.wire.usage.input_tokens),
        output_tokens: wire
            .usage
            .output_tokens
            .saturating_add(fallback.wire.usage.output_tokens),
    };
    wire.model = fallback.wire.model;
    let mut meta = IndexMap::new();
    for id in wire.answers.keys() {
        let hop = if retry.iter().any(|retry_id| retry_id == id) {
            CascadeHop::Fallback
        } else {
            CascadeHop::First
        };
        meta.insert(
            id.clone(),
            AnswerMeta {
                cascade_hop: Some(hop),
                ..AnswerMeta::default()
            },
        );
    }
    Evaluated {
        wire,
        meta,
        backend_id: format!("cascade:{}+{}", first.backend_id, fallback.backend_id),
    }
}
