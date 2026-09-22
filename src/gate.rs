use std::time::Instant;

use indexmap::IndexMap;

use crate::answer::{ChoiceAnswer, NoulAnswer, ScoreAnswer};
use crate::backend::{Backend, Client, wire_question};
use crate::battery::shipped_questions;
use crate::error::{DecodeError, Error, PolicyError};
use crate::ids::{ActionId, QuestionId};
use crate::policy::{
    EffectiveGates, Fail, HarmClass, Policy, UnsureVerdict, effective_gates, verdict_from_signal,
};
use crate::question::Question;
use crate::state::{PreparedCall, State};
use crate::verdict::{ActionHint, Decision, UnsureReason, Verdict, hint};
use crate::wire::{self, Usage, WireAnswer, WireRequest};

/// Prepared tool call. The host runs path checks and deny-lists before `gate()`.
/// `Auto` means snapif has no objection. The host still decides whether to execute.
pub struct GateRequest {
    pub action_id: ActionId,
    pub prepared: PreparedCall,
    pub state: State,
    pub extra_questions: Vec<Question>,
}

impl<B: Backend> Client<B> {
    /// Action path. Not observation.
    ///
    /// The host checks paths and deny-lists before this call. `Verdict::Auto`
    /// means no objection from the shipped policy.
    pub async fn gate(&self, req: GateRequest) -> Result<Verdict, Error> {
        if req.action_id.0.is_empty() {
            return Err(Error::EmptyActionId);
        }
        let policy = self
            .policy
            .as_ref()
            .ok_or_else(|| Error::Policy(PolicyError::Invariant("policy".to_string())))?;
        let gates = effective_gates(policy, &req.action_id, None)?;
        let mut wire_questions = IndexMap::new();
        for question in shipped_questions() {
            let (id, wire) = wire_question(&question);
            wire_questions.insert(id, wire);
        }
        for question in &req.extra_questions {
            let (id, wire) = wire_question(question);
            if wire_questions.contains_key(&id) {
                return Ok(self.closed(
                    &req.action_id,
                    &gates,
                    UnsureReason::Wire,
                    Usage::default(),
                    self.backend.id(),
                    IndexMap::new(),
                ));
            }
            wire_questions.insert(id, wire);
        }
        let mut request = WireRequest {
            model: self.model.clone(),
            state: req.state.to_wire(Some(&req.prepared)),
            questions: wire_questions,
        };
        let encoded = match wire::encode(&request) {
            Ok(encoded) => encoded,
            Err(_) => {
                return Ok(self.closed(
                    &req.action_id,
                    &gates,
                    UnsureReason::Wire,
                    Usage::default(),
                    self.backend.id(),
                    IndexMap::new(),
                ));
            }
        };
        let mut truncated = false;
        if encoded.truncated_untrusted {
            request = match wire::decode_request(&encoded.body) {
                Ok(request) => request,
                Err(_) => {
                    return Ok(self.closed(
                        &req.action_id,
                        &gates,
                        UnsureReason::Wire,
                        Usage::default(),
                        self.backend.id(),
                        IndexMap::new(),
                    ));
                }
            };
            truncated = true;
        }
        let deadline = Instant::now() + self.timeout;
        let evaluated = match self.backend.evaluate(request.clone(), deadline).await {
            Ok(evaluated) => evaluated,
            Err(_) => {
                return Ok(self.closed(
                    &req.action_id,
                    &gates,
                    UnsureReason::Backend,
                    Usage::default(),
                    self.backend.id(),
                    IndexMap::new(),
                ));
            }
        };
        if let Some(on_usage) = &self.on_usage {
            on_usage(evaluated.wire.usage);
        }
        let mut meta = evaluated.meta;
        for (id, answer) in &evaluated.wire.answers {
            crate::backend::record_prob_sum(&mut meta, id, answer);
        }
        if let Err(err) = wire::check_response(&request.questions, &evaluated.wire) {
            return Ok(self.closed(
                &req.action_id,
                &gates,
                UnsureReason::Decode(err),
                evaluated.wire.usage,
                &evaluated.backend_id,
                meta,
            ));
        }
        let harm = match harm_choice(&evaluated.wire.answers, policy.choice.signal) {
            Ok(harm) => harm,
            Err(err) => {
                return Ok(self.closed(
                    &req.action_id,
                    &gates,
                    UnsureReason::Decode(err),
                    evaluated.wire.usage,
                    &evaluated.backend_id,
                    meta,
                ));
            }
        };
        let gates = effective_gates(policy, &req.action_id, Some(harm.class))?;
        let mut reasons = Vec::new();
        if let Some((from, to)) = gates.harm_bumped {
            reasons.push(UnsureReason::HarmClassBump { from, to });
        }
        if crate::backends::cascade::fallback_still_below(
            &meta,
            &evaluated.wire.answers,
            policy.cascade_min,
        ) {
            reasons.push(UnsureReason::CascadeStillUnsure);
        }
        match block_hit(policy, &req.action_id, &gates, &evaluated.wire.answers) {
            Err(err) => {
                return Ok(self.closed(
                    &req.action_id,
                    &gates,
                    UnsureReason::Decode(err),
                    evaluated.wire.usage,
                    &evaluated.backend_id,
                    meta,
                ));
            }
            Ok(Some(verdict)) => {
                return Ok(self.finish(
                    push_reasons(verdict, reasons),
                    evaluated.wire.usage,
                    evaluated.backend_id,
                    self.shadow_on(policy.shadow),
                    Some(harm.label),
                    meta,
                ));
            }
            Ok(None) => {}
        }
        let mut verdict = verdict_from_signal(harm.signal, &gates, req.action_id.clone());
        verdict = match fold_unsure_blocks(
            verdict,
            &req.action_id,
            &gates,
            policy,
            &evaluated.wire.answers,
        ) {
            Ok(verdict) => verdict,
            Err(err) => {
                return Ok(self.closed(
                    &req.action_id,
                    &gates,
                    UnsureReason::Decode(err),
                    evaluated.wire.usage,
                    &evaluated.backend_id,
                    meta,
                ));
            }
        };
        verdict = fold_extras(
            verdict,
            &req.action_id,
            policy,
            &gates,
            &req.extra_questions,
            &evaluated.wire.answers,
        );
        if truncated {
            verdict = absorb(
                verdict,
                when_unsure_bump(&gates, req.action_id.clone(), UnsureReason::Truncated),
                req.action_id.clone(),
            );
        }
        Ok(self.finish(
            push_reasons(verdict, reasons),
            evaluated.wire.usage,
            evaluated.backend_id,
            self.shadow_on(policy.shadow),
            Some(harm.label),
            meta,
        ))
    }

    fn closed(
        &self,
        action_id: &ActionId,
        gates: &EffectiveGates,
        reason: UnsureReason,
        usage: Usage,
        backend_id: &str,
        meta: IndexMap<String, crate::backend::AnswerMeta>,
    ) -> Verdict {
        let shadow = self.shadow_on(self.policy.as_ref().is_some_and(|policy| policy.shadow));
        let verdict = if self.fail_mode() == Fail::Open && gates.auto.is_some() {
            Verdict::Review(hint(action_id.clone(), vec![reason]))
        } else {
            Verdict::Escalate(hint(action_id.clone(), vec![reason]))
        };
        self.finish(verdict, usage, backend_id.to_string(), shadow, None, meta)
    }

    fn finish(
        &self,
        verdict: Verdict,
        usage: Usage,
        backend_id: String,
        shadow: bool,
        guess: Option<String>,
        meta: IndexMap<String, crate::backend::AnswerMeta>,
    ) -> Verdict {
        map_hint(verdict, |mut hint| {
            hint.usage = usage;
            hint.backend_id = backend_id;
            hint.shadow = shadow;
            if hint.guess.is_none() {
                hint.guess = guess;
            }
            hint.meta = meta;
            hint
        })
    }
}

fn push_reasons(verdict: Verdict, extra: Vec<UnsureReason>) -> Verdict {
    if extra.is_empty() {
        return verdict;
    }
    map_hint(verdict, |mut hint| {
        hint.reasons.extend(extra);
        hint
    })
}

struct HarmObs {
    class: HarmClass,
    label: String,
    signal: f64,
}

fn harm_choice(
    answers: &IndexMap<String, WireAnswer>,
    signal: crate::policy::Signal,
) -> Result<HarmObs, DecodeError> {
    let Some(WireAnswer::Choice {
        choice,
        probabilities,
        confidence,
    }) = answers.get("harm_class")
    else {
        return Err(DecodeError::MissingAnswer {
            key: QuestionId::new("harm_class"),
        });
    };
    let class = parse_harm(choice).ok_or_else(|| DecodeError::UnknownLabel {
        key: QuestionId::new("harm_class"),
        label: choice.clone(),
    })?;
    let answer = ChoiceAnswer {
        label: choice.clone(),
        confidence: *confidence,
        probabilities: probabilities.clone(),
    };
    let signal = answer.signal(signal);
    Ok(HarmObs {
        class,
        label: choice.clone(),
        signal,
    })
}

fn parse_harm(label: &str) -> Option<HarmClass> {
    match label {
        "none" => Some(HarmClass::None),
        "read" => Some(HarmClass::Read),
        "write" => Some(HarmClass::Write),
        "exec" => Some(HarmClass::Exec),
        "network" => Some(HarmClass::Network),
        "money" => Some(HarmClass::Money),
        _ => None,
    }
}

fn block_answer(
    answers: &IndexMap<String, WireAnswer>,
    id: &QuestionId,
) -> Result<f64, DecodeError> {
    match answers.get(&id.0) {
        Some(WireAnswer::Noul { noul }) => Ok(*noul),
        Some(_) => Err(DecodeError::TypeMismatch { key: id.clone() }),
        None => Err(DecodeError::MissingAnswer { key: id.clone() }),
    }
}

fn block_hit(
    policy: &Policy,
    action_id: &ActionId,
    gates: &EffectiveGates,
    answers: &IndexMap<String, WireAnswer>,
) -> Result<Option<Verdict>, DecodeError> {
    for block in &gates.block_on {
        let noul = block_answer(answers, &block.id)?;
        let decision = NoulAnswer { p: noul }.decide(&policy.noul, Some(block));
        let fires = matches!(
            (block.when, &decision),
            (crate::policy::BlockWhen::Yes, Decision::Known(true))
                | (crate::policy::BlockWhen::No, Decision::Known(false))
                | (crate::policy::BlockWhen::Unsure, Decision::Unsure { .. })
        );
        if fires {
            return Ok(Some(Verdict::Escalate(hint(
                action_id.clone(),
                vec![UnsureReason::Battery {
                    id: block.id.clone(),
                    when: block.when,
                }],
            ))));
        }
    }
    Ok(None)
}

fn fold_unsure_blocks(
    mut verdict: Verdict,
    action_id: &ActionId,
    gates: &EffectiveGates,
    policy: &Policy,
    answers: &IndexMap<String, WireAnswer>,
) -> Result<Verdict, DecodeError> {
    for block in &gates.block_on {
        if matches!(block.when, crate::policy::BlockWhen::Unsure) {
            continue;
        }
        let noul = block_answer(answers, &block.id)?;
        let decision = NoulAnswer { p: noul }.decide(&policy.noul, Some(block));
        if matches!(decision, Decision::Unsure { .. }) {
            let bump = when_unsure_bump(
                gates,
                action_id.clone(),
                UnsureReason::Battery {
                    id: block.id.clone(),
                    when: block.when,
                },
            );
            verdict = absorb(verdict, bump, action_id.clone());
        }
    }
    Ok(verdict)
}

fn fold_extras(
    mut verdict: Verdict,
    action_id: &ActionId,
    policy: &Policy,
    gates: &EffectiveGates,
    extras: &[Question],
    answers: &IndexMap<String, WireAnswer>,
) -> Verdict {
    let blocked: Vec<&str> = gates
        .block_on
        .iter()
        .map(|block| block.id.0.as_str())
        .collect();
    for question in extras {
        let id = question.id().0.as_str();
        if blocked.contains(&id) {
            continue;
        }
        let Some(answer) = answers.get(id) else {
            continue;
        };
        let extra = match (question, answer) {
            (
                Question::Choice(_),
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
                verdict_from_signal(
                    decoded.signal(policy.choice.signal),
                    gates,
                    action_id.clone(),
                )
            }
            (
                Question::Score(_),
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
                verdict_from_signal(
                    decoded.signal(policy.choice.signal),
                    gates,
                    action_id.clone(),
                )
            }
            (Question::Noul(_), WireAnswer::Noul { noul }) => {
                let decision = NoulAnswer { p: *noul }.decide(&policy.noul, None);
                if matches!(decision, Decision::Unsure { .. }) {
                    when_unsure_bump(
                        gates,
                        action_id.clone(),
                        UnsureReason::NoulBand { noul: *noul },
                    )
                } else {
                    continue;
                }
            }
            _ => continue,
        };
        verdict = absorb(verdict, extra, action_id.clone());
    }
    verdict
}

fn when_unsure_bump(gates: &EffectiveGates, action_id: ActionId, reason: UnsureReason) -> Verdict {
    match gates.when_unsure {
        UnsureVerdict::Escalate => Verdict::Escalate(hint(action_id, vec![reason])),
        UnsureVerdict::ReviewGuess => Verdict::Review(hint(action_id, vec![reason])),
    }
}

fn absorb(current: Verdict, extra: Verdict, action_id: ActionId) -> Verdict {
    let rank = match &current {
        Verdict::Auto(_) => 0,
        Verdict::Review(_) => 1,
        Verdict::Escalate(_) => 2,
    }
    .max(match &extra {
        Verdict::Auto(_) => 0,
        Verdict::Review(_) => 1,
        Verdict::Escalate(_) => 2,
    });
    let extra_reasons = take_hint(extra).reasons;
    let mut hint = take_hint(current);
    hint.action_id = action_id;
    hint.reasons.extend(extra_reasons);
    match rank {
        0 => Verdict::Auto(hint),
        1 => Verdict::Review(hint),
        _ => Verdict::Escalate(hint),
    }
}

fn take_hint(verdict: Verdict) -> ActionHint {
    match verdict {
        Verdict::Auto(hint) | Verdict::Review(hint) | Verdict::Escalate(hint) => hint,
    }
}

fn map_hint(verdict: Verdict, map: impl FnOnce(ActionHint) -> ActionHint) -> Verdict {
    match verdict {
        Verdict::Auto(hint) => Verdict::Auto(map(hint)),
        Verdict::Review(hint) => Verdict::Review(map(hint)),
        Verdict::Escalate(hint) => Verdict::Escalate(map(hint)),
    }
}

#[cfg(test)]
mod tests {
    use super::GateRequest;
    use crate::Client;
    use crate::backends::fake::FakeBackend;
    use crate::ids::ActionId;
    use crate::policy::Policy;
    use crate::state::{PreparedCall, State};
    use serde_json::json;

    #[test]
    fn gate_posts_the_client_model() {
        let mut backend = FakeBackend::new().on_choice("harm_class", "read", 0.91);
        for id in crate::backends::cascade::battery_ids() {
            if id.0 == "harm_class" {
                continue;
            }
            backend = backend.on_noul(&id.0, 0.0);
        }
        let mut client = Client::new(backend).policy(Policy::shipped("tool-gate").expect("policy"));
        client.model = "custom-model".to_string();
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
            extra_questions: vec![],
        }))
        .expect("gate");
        assert!(matches!(verdict, crate::verdict::Verdict::Auto(_)));
        assert_eq!(
            client.backend().last_model().as_deref(),
            Some("custom-model")
        );
    }
}
