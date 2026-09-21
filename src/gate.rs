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
            wire_questions.insert(id, wire);
        }
        let mut request = WireRequest {
            model: "jev-latest".to_string(),
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
                ));
            }
        };
        let mut truncated = false;
        if encoded.truncated_untrusted {
            request = wire::decode_request(&encoded.body)?;
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
                ));
            }
        };
        if let Some(on_usage) = &self.on_usage {
            on_usage(evaluated.wire.usage);
        }
        if let Err(err) = wire::check_response(&request.questions, &evaluated.wire) {
            return Ok(self.closed(
                &req.action_id,
                &gates,
                UnsureReason::Decode(err),
                evaluated.wire.usage,
                &evaluated.backend_id,
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
                ));
            }
        };
        let gates = effective_gates(policy, &req.action_id, Some(harm.class))?;
        let mut reasons = Vec::new();
        if let Some((from, to)) = gates.harm_bumped {
            reasons.push(UnsureReason::HarmClassBump { from, to });
        }
        if truncated {
            reasons.push(UnsureReason::Truncated);
        }
        if let Some(verdict) = block_hit(policy, &req.action_id, &gates, &evaluated.wire.answers) {
            return Ok(self.finish(
                verdict,
                evaluated.wire.usage,
                evaluated.backend_id,
                policy.shadow,
                Some(harm.label),
                reasons,
            ));
        }
        let mut verdict = verdict_from_signal(harm.signal, &gates, req.action_id.clone());
        verdict = fold_unsure_blocks(verdict, &gates, policy, &evaluated.wire.answers);
        verdict = fold_extras(
            verdict,
            policy,
            &gates,
            &req.extra_questions,
            &evaluated.wire.answers,
        );
        Ok(self.finish(
            verdict,
            evaluated.wire.usage,
            evaluated.backend_id,
            policy.shadow,
            Some(harm.label),
            reasons,
        ))
    }

    fn closed(
        &self,
        action_id: &ActionId,
        gates: &EffectiveGates,
        reason: UnsureReason,
        usage: Usage,
        backend_id: &str,
    ) -> Verdict {
        let shadow = self.policy.as_ref().is_some_and(|policy| policy.shadow);
        let verdict = if self
            .policy
            .as_ref()
            .is_some_and(|policy| policy.fail == Fail::Open)
            && gates.auto.is_some()
        {
            Verdict::Review(hint(action_id.clone(), vec![reason]))
        } else {
            Verdict::Escalate(hint(action_id.clone(), vec![reason]))
        };
        self.finish(
            verdict,
            usage,
            backend_id.to_string(),
            shadow,
            None,
            Vec::new(),
        )
    }

    fn finish(
        &self,
        verdict: Verdict,
        usage: Usage,
        backend_id: String,
        shadow: bool,
        guess: Option<String>,
        extra: Vec<UnsureReason>,
    ) -> Verdict {
        map_hint(verdict, |mut hint| {
            hint.usage = usage;
            hint.backend_id = backend_id;
            hint.shadow = shadow;
            if hint.guess.is_none() {
                hint.guess = guess;
            }
            hint.reasons.extend(extra);
            hint
        })
    }
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

fn block_hit(
    policy: &Policy,
    action_id: &ActionId,
    gates: &EffectiveGates,
    answers: &IndexMap<String, WireAnswer>,
) -> Option<Verdict> {
    for block in &gates.block_on {
        let Some(WireAnswer::Noul { noul }) = answers.get(&block.id.0) else {
            continue;
        };
        let decision = NoulAnswer { p: *noul }.decide(&policy.noul, Some(block));
        let fires = matches!(
            (block.when, &decision),
            (crate::policy::BlockWhen::Yes, Decision::Known(true))
                | (crate::policy::BlockWhen::No, Decision::Known(false))
                | (crate::policy::BlockWhen::Unsure, Decision::Unsure { .. })
        );
        if fires {
            return Some(Verdict::Escalate(hint(
                action_id.clone(),
                vec![UnsureReason::Battery {
                    id: block.id.clone(),
                    when: block.when,
                }],
            )));
        }
    }
    None
}

fn fold_unsure_blocks(
    mut verdict: Verdict,
    gates: &EffectiveGates,
    policy: &Policy,
    answers: &IndexMap<String, WireAnswer>,
) -> Verdict {
    for block in &gates.block_on {
        if matches!(block.when, crate::policy::BlockWhen::Unsure) {
            continue;
        }
        let Some(WireAnswer::Noul { noul }) = answers.get(&block.id.0) else {
            continue;
        };
        let decision = NoulAnswer { p: *noul }.decide(&policy.noul, Some(block));
        if matches!(decision, Decision::Unsure { .. }) {
            verdict = max_severity(verdict, when_unsure_verdict(gates));
        }
    }
    verdict
}

fn fold_extras(
    mut verdict: Verdict,
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
        let id = question_id(question);
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
                    ActionId::new(id),
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
                    ActionId::new(id),
                )
            }
            (Question::Noul(_), WireAnswer::Noul { noul }) => {
                let decision = NoulAnswer { p: *noul }.decide(&policy.noul, None);
                if matches!(decision, Decision::Unsure { .. }) {
                    when_unsure_verdict(gates)
                } else {
                    continue;
                }
            }
            _ => continue,
        };
        verdict = max_severity(verdict, extra);
    }
    verdict
}

fn when_unsure_verdict(gates: &EffectiveGates) -> Verdict {
    let action_id = ActionId::new("extra");
    let reason = UnsureReason::BelowFloor {
        confidence: 0.0,
        floor: gates.escalate_below,
    };
    match gates.when_unsure {
        UnsureVerdict::Escalate => Verdict::Escalate(hint(action_id, vec![reason])),
        UnsureVerdict::ReviewGuess => Verdict::Review(hint(action_id, vec![reason])),
    }
}

fn question_id(question: &Question) -> &str {
    match question {
        Question::Choice(choice) => choice.id.0.as_str(),
        Question::Score(score) => score.id.0.as_str(),
        Question::Noul(noul) => noul.id.0.as_str(),
    }
}

fn max_severity(current: Verdict, candidate: Verdict) -> Verdict {
    if rank(&candidate) > rank(&current) {
        candidate
    } else {
        current
    }
}

fn rank(verdict: &Verdict) -> u8 {
    match verdict {
        Verdict::Auto(_) => 0,
        Verdict::Review(_) => 1,
        Verdict::Escalate(_) => 2,
    }
}

fn map_hint(verdict: Verdict, map: impl FnOnce(ActionHint) -> ActionHint) -> Verdict {
    match verdict {
        Verdict::Auto(hint) => Verdict::Auto(map(hint)),
        Verdict::Review(hint) => Verdict::Review(map(hint)),
        Verdict::Escalate(hint) => Verdict::Escalate(map(hint)),
    }
}
