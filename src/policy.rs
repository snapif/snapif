use crate::answer::NoulAnswer;
use crate::error::{Error, PolicyError};
use crate::ids::{ActionId, BatteryId, QuestionId};
use crate::verdict::{Decision, UnsureReason, Verdict, hint};
use indexmap::IndexMap;
use serde::Deserialize;

fn blank_action_id() -> ActionId {
    ActionId::new("")
}

fn default_review_below() -> f64 {
    1.0
}

fn default_signal() -> Signal {
    Signal::Confidence
}

fn default_yes_auto() -> f64 {
    0.90
}

fn default_no_auto() -> f64 {
    0.10
}

fn default_cascade_min() -> f64 {
    0.80
}

fn default_battery() -> BatteryId {
    BatteryId::new("tool-gate")
}

/// Thresholds for `gate` and `ask`.
///
/// `shipped`, `from_toml_str`, and `load` are the constructors. They run
/// `finish`, which is the only place that sets `sealed`. A struct literal
/// outside this module cannot name that field.
#[derive(Debug, Clone, PartialEq, serde::Serialize, Deserialize)]
pub struct Policy {
    #[serde(default = "default_schema")]
    pub schema_version: u32,
    #[serde(default)]
    pub fail: Fail,
    #[serde(default)]
    pub shadow: bool,
    #[serde(default = "default_cascade_min")]
    pub cascade_min: f64,
    pub choice: ChoiceGates,
    #[serde(default)]
    pub noul: NoulPolicy,
    #[serde(default)]
    pub default_action: Option<ActionPolicy>,
    #[serde(default)]
    pub actions: IndexMap<ActionId, ActionPolicy>,
    #[serde(default = "default_battery")]
    pub battery: BatteryId,
    /// Set only by `finish` after the invariant checks.
    #[serde(skip)]
    sealed: bool,
}

fn default_schema() -> u32 {
    1
}

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, Deserialize)]
pub struct ChoiceGates {
    pub escalate_below: f64,
    #[serde(default = "default_review_below")]
    pub review_below: f64,
    #[serde(default = "default_signal")]
    pub signal: Signal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Signal {
    Confidence,
    TopProb,
    Margin,
}

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, Deserialize)]
pub struct NoulPolicy {
    #[serde(default = "default_yes_auto")]
    pub yes_auto: f64,
    #[serde(default = "default_no_auto")]
    pub no_auto: f64,
}

impl Default for NoulPolicy {
    fn default() -> Self {
        Self {
            yes_auto: default_yes_auto(),
            no_auto: default_no_auto(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, Deserialize)]
pub struct ActionPolicy {
    #[serde(default = "blank_action_id", skip_deserializing)]
    pub action_id: ActionId,
    #[serde(default)]
    pub auto: Option<f64>,
    pub review: f64,
    pub when_unsure: UnsureVerdict,
    pub class: HarmClass,
    #[serde(default)]
    pub block_on: Vec<Block>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, Deserialize)]
pub struct Block {
    pub id: QuestionId,
    #[serde(default)]
    pub when: BlockWhen,
    #[serde(default)]
    pub yes_auto: Option<f64>,
    #[serde(default)]
    pub no_auto: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlockWhen {
    #[default]
    Yes,
    No,
    Unsure,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HarmClass {
    None,
    Read,
    Write,
    Exec,
    Network,
    Money,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Fail {
    #[default]
    Closed,
    Open,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnsureVerdict {
    ReviewGuess,
    Escalate,
}

pub struct EffectiveGates {
    pub escalate_below: f64,
    pub auto: Option<f64>,
    pub review: f64,
    pub when_unsure: UnsureVerdict,
    pub block_on: Vec<Block>,
    pub harm_bumped: Option<(HarmClass, HarmClass)>,
}

#[derive(Debug, Clone, Copy)]
pub struct NoulObs<'a> {
    pub id: &'a str,
    pub p: f64,
}

impl Policy {
    pub fn shipped(name: &str) -> Result<Self, PolicyError> {
        let raw = match name {
            "tool-gate" => include_str!("../policies/tool-gate.toml"),
            "triage" => include_str!("../policies/triage.toml"),
            other => {
                return Err(PolicyError::Invariant(format!("unknown policy {other}")));
            }
        };
        Self::from_toml_str(raw)
    }

    /// Shipped id, or a path whose name ends in `.toml`.
    pub fn load(spec: &str) -> Result<Self, Error> {
        if spec.ends_with(".toml") {
            let text = std::fs::read_to_string(spec)?;
            Ok(Self::from_toml_str(&text)?)
        } else {
            Ok(Self::shipped(spec)?)
        }
    }

    pub fn from_toml_str(raw: &str) -> Result<Self, PolicyError> {
        let policy: Self =
            toml::from_str(raw).map_err(|err| PolicyError::Invariant(err.to_string()))?;
        policy.finish()
    }

    fn finish(mut self) -> Result<Self, PolicyError> {
        self.pre_invariant_checks()?;
        let Some(mut default_action) = self.default_action.take() else {
            return Err(PolicyError::Invariant("default_action".to_string()));
        };
        default_action.action_id = ActionId::new("default");
        self.default_action = Some(default_action);
        for (key, row) in &mut self.actions {
            row.action_id = key.clone();
        }
        self.check_invariants()?;
        self.sealed = true;
        Ok(self)
    }

    pub(crate) fn ensure_checked(&self) -> Result<(), PolicyError> {
        if !self.sealed {
            return Err(PolicyError::Invariant("unchecked policy".to_string()));
        }
        self.pre_invariant_checks()?;
        self.check_invariants()
    }

    fn pre_invariant_checks(&self) -> Result<(), PolicyError> {
        if self.schema_version != 1 {
            return Err(PolicyError::Schema(self.schema_version));
        }
        if self.choice.escalate_below == 0.0 {
            return Err(PolicyError::MissingUnsure);
        }
        Ok(())
    }

    fn check_invariants(&self) -> Result<(), PolicyError> {
        let e = self.choice.escalate_below;
        in_unit("escalate_below", e)?;
        in_unit("review_below", self.choice.review_below)?;
        if self.choice.review_below < e {
            return Err(PolicyError::Invariant(
                "review_below >= escalate_below".to_string(),
            ));
        }
        in_unit("yes_auto", self.noul.yes_auto)?;
        in_unit("no_auto", self.noul.no_auto)?;
        if self.noul.yes_auto < self.noul.no_auto {
            return Err(PolicyError::Invariant("yes_auto >= no_auto".to_string()));
        }
        in_unit("cascade_min", self.cascade_min)?;
        let cascade_floor = e
            .max(2.0 * self.noul.yes_auto - 1.0)
            .max(1.0 - 2.0 * self.noul.no_auto);
        if self.cascade_min + 1e-12 < cascade_floor {
            return Err(PolicyError::Invariant(
                "cascade_min is below its floor".to_string(),
            ));
        }
        let Some(default_action) = self.default_action.as_ref() else {
            return Err(PolicyError::Invariant("default_action".to_string()));
        };
        check_action(e, self.choice.review_below, default_action)?;
        for action in self.actions.values() {
            check_action(e, self.choice.review_below, action)?;
        }
        Ok(())
    }
}

fn in_unit(name: &str, value: f64) -> Result<(), PolicyError> {
    if (0.0..=1.0).contains(&value) {
        Ok(())
    } else {
        Err(PolicyError::Invariant(format!("{name} must be in 0..=1")))
    }
}

fn check_action(
    escalate_below: f64,
    review_below: f64,
    action: &ActionPolicy,
) -> Result<(), PolicyError> {
    in_unit("review", action.review)?;
    if action.review < escalate_below {
        return Err(PolicyError::Invariant(
            "review >= escalate_below".to_string(),
        ));
    }
    if let Some(auto) = action.auto {
        in_unit("auto", auto)?;
        let mut auto_eff = auto.max(escalate_below);
        if action.class >= HarmClass::Write {
            auto_eff = auto_eff.max(review_below);
        }
        if auto_eff + 1e-12 < action.review {
            return Err(PolicyError::Invariant("auto_eff >= review".to_string()));
        }
    }
    for block in &action.block_on {
        if let Some(yes) = block.yes_auto {
            in_unit("block.yes_auto", yes)?;
        }
        if let Some(no) = block.no_auto {
            in_unit("block.no_auto", no)?;
        }
        if let (Some(yes), Some(no)) = (block.yes_auto, block.no_auto)
            && yes < no
        {
            return Err(PolicyError::Invariant(
                "block yes_auto >= no_auto".to_string(),
            ));
        }
    }
    Ok(())
}

pub fn effective_gates(
    policy: &Policy,
    action_id: &ActionId,
    harm_class: Option<HarmClass>,
) -> Result<EffectiveGates, PolicyError> {
    policy.ensure_checked()?;
    let Some(default_action) = policy.default_action.as_ref() else {
        return Err(PolicyError::UnknownAction(action_id.clone()));
    };
    let action = policy.actions.get(action_id).unwrap_or(default_action);
    let escalate_below = policy.choice.escalate_below;
    let mut auto = action.auto.map(|value| value.max(escalate_below));
    if action.class >= HarmClass::Write {
        auto = auto.map(|value| value.max(policy.choice.review_below));
    }
    let mut harm_bumped = None;
    if let Some(decoded) = harm_class
        && decoded > action.class
    {
        auto = None;
        harm_bumped = Some((action.class, decoded));
    }
    Ok(EffectiveGates {
        escalate_below,
        auto,
        review: action.review.max(escalate_below),
        when_unsure: action.when_unsure,
        block_on: action.block_on.clone(),
        harm_bumped,
    })
}

pub fn verdict_from_signal(s: f64, gates: &EffectiveGates, action_id: ActionId) -> Verdict {
    if s < gates.escalate_below {
        let verdict = match gates.when_unsure {
            UnsureVerdict::ReviewGuess => Verdict::Review,
            UnsureVerdict::Escalate => Verdict::Escalate,
        };
        return verdict(hint(
            action_id,
            vec![UnsureReason::BelowFloor {
                confidence: s,
                floor: gates.escalate_below,
            }],
        ));
    }
    if let Some(auto) = gates.auto
        && s >= auto
    {
        return Verdict::Auto(hint(action_id, Vec::new()));
    }
    if s >= gates.review {
        Verdict::Review(hint(
            action_id,
            vec![UnsureReason::ReviewFloor {
                confidence: s,
                floor: gates.review,
                auto: gates.auto,
            }],
        ))
    } else {
        Verdict::Escalate(hint(
            action_id,
            vec![UnsureReason::BelowAuto {
                confidence: s,
                auto: gates.auto.unwrap_or(gates.review),
            }],
        ))
    }
}

/// Synthetic signal helper. A block id with no row in `nouls` stays inert.
/// `gate` does not use this path: a missing block answer there is a decode failure.
pub fn verdict_with_blocks(
    policy: &Policy,
    action_id: &ActionId,
    s: f64,
    harm_class: Option<HarmClass>,
    nouls: &[NoulObs<'_>],
) -> Result<Verdict, PolicyError> {
    let gates = effective_gates(policy, action_id, harm_class)?;
    for block in &gates.block_on {
        let Some(obs) = nouls.iter().find(|row| row.id == block.id.0) else {
            continue;
        };
        let decision = NoulAnswer { p: obs.p }.decide(&policy.noul, Some(block));
        let fires = matches!(
            (block.when, &decision),
            (BlockWhen::Yes, Decision::Known(true))
                | (BlockWhen::No, Decision::Known(false))
                | (BlockWhen::Unsure, Decision::Unsure { .. })
        );
        if fires {
            return Ok(Verdict::Escalate(hint(
                action_id.clone(),
                vec![UnsureReason::Battery {
                    id: block.id.clone(),
                    when: block.when,
                }],
            )));
        }
    }
    Ok(verdict_from_signal(s, &gates, action_id.clone()))
}

#[cfg(test)]
mod tests {
    use super::{Policy, effective_gates};
    use crate::ids::ActionId;

    #[test]
    fn raw_toml_is_unchecked_until_finish() {
        let raw = include_str!("../policies/tool-gate.toml");
        let policy: Policy = toml::from_str(raw).expect("toml");
        assert!(!policy.sealed);
        match effective_gates(&policy, &ActionId::new("tag"), None) {
            Err(err) => assert!(err.to_string().contains("unchecked"), "{err}"),
            Ok(_) => panic!("raw toml must not pass effective_gates"),
        }

        let checked = Policy::from_toml_str(raw).expect("finish");
        assert!(checked.sealed);
        effective_gates(&checked, &ActionId::new("tag"), None).expect("sealed policy");
    }
}
