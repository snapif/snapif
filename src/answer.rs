use crate::error::DecodeError;
use crate::ids::QuestionId;
use crate::policy::{Block, NoulPolicy, Signal};
use crate::question::{ChoiceLabels, ScoreLabels};
use crate::verdict::{Decision, UnsureReason};
use crate::wire::renormalize_probabilities;
use indexmap::IndexMap;

pub struct NoulAnswer {
    pub p: f64,
}

impl NoulAnswer {
    pub fn decide(&self, pol: &NoulPolicy, block: Option<&Block>) -> Decision<bool> {
        let yes = block.and_then(|row| row.yes_auto).unwrap_or(pol.yes_auto);
        let no = block.and_then(|row| row.no_auto).unwrap_or(pol.no_auto);
        if self.p >= yes {
            Decision::Known(true)
        } else if self.p <= no {
            Decision::Known(false)
        } else {
            Decision::Unsure {
                reason: UnsureReason::NoulBand { noul: self.p },
                guess: Some(self.p >= 0.5),
            }
        }
    }
}

pub struct ChoiceAnswer {
    pub label: String,
    pub confidence: f64,
    pub probabilities: IndexMap<String, f64>,
}

impl ChoiceAnswer {
    pub fn signal(&self, signal: Signal) -> f64 {
        match signal {
            Signal::Confidence => self.confidence,
            Signal::TopProb => top_prob(&self.probabilities),
            Signal::Margin => margin(&self.probabilities),
        }
    }

    pub fn decide<T: ChoiceLabels>(&self, s: f64, floor: f64) -> Result<Decision<T>, DecodeError> {
        let decoded = T::from_label(&self.label);
        if s < floor {
            return Ok(Decision::Unsure {
                reason: UnsureReason::BelowFloor {
                    confidence: s,
                    floor,
                },
                guess: decoded,
            });
        }
        match decoded {
            Some(value) => Ok(Decision::Known(value)),
            None => Err(DecodeError::UnknownLabel {
                key: QuestionId::new(&self.label),
                label: self.label.clone(),
            }),
        }
    }
}

pub struct ScoreAnswer {
    pub score: f64,
    pub confidence: f64,
    pub probabilities: IndexMap<String, f64>,
}

impl ScoreAnswer {
    pub fn signal(&self, signal: Signal) -> f64 {
        match signal {
            Signal::Confidence => self.confidence,
            Signal::TopProb => top_prob(&self.probabilities),
            Signal::Margin => margin(&self.probabilities),
        }
    }

    pub fn decide<T: ScoreLabels>(&self, s: f64, floor: f64) -> Result<Decision<T>, DecodeError> {
        let guess = index_of(self.score).and_then(T::from_index);
        if s < floor {
            return Ok(Decision::Unsure {
                reason: UnsureReason::BelowFloor {
                    confidence: s,
                    floor,
                },
                guess,
            });
        }
        match guess {
            Some(value) => Ok(Decision::Known(value)),
            None => Err(DecodeError::OutOfRange {
                key: QuestionId::new("score"),
            }),
        }
    }
}

fn index_of(score: f64) -> Option<usize> {
    if score.is_finite() && score >= 0.0 {
        Some(score.round() as usize)
    } else {
        None
    }
}

fn top_prob(probabilities: &IndexMap<String, f64>) -> f64 {
    let (scaled, _) = renormalize_probabilities(probabilities);
    scaled.values().copied().fold(0.0, f64::max)
}

fn margin(probabilities: &IndexMap<String, f64>) -> f64 {
    let (scaled, _) = renormalize_probabilities(probabilities);
    if scaled.len() <= 1 {
        return 1.0;
    }
    let mut top1 = 0.0;
    let mut top2 = 0.0;
    for value in scaled.values().copied() {
        if value >= top1 {
            top2 = top1;
            top1 = value;
        } else if value > top2 {
            top2 = value;
        }
    }
    top1 - top2
}
