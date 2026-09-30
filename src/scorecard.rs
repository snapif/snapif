//! Labeled-score totals for `snapif calibrate`.

#[derive(Debug, Default, Clone)]
pub struct Scorecard {
    brier_sum: f64,
    brier_n: u64,
    bins: [u64; 5],
    bin_hits: [u64; 5],
    choice_hit: u64,
    choice_n: u64,
    choice_known: u64,
    choice_guess: u64,
}

impl Scorecard {
    pub fn add_noul(&mut self, probability: f64, truth: bool) {
        let y = if truth { 1.0 } else { 0.0 };
        let err = probability - y;
        self.brier_sum += err * err;
        self.brier_n += 1;
        let bin = (probability.clamp(0.0, 0.999) * 5.0) as usize;
        self.bins[bin] += 1;
        if truth {
            self.bin_hits[bin] += 1;
        }
    }

    pub fn add_choice(&mut self, matched: bool) {
        self.add_choice_compared(matched, false);
    }

    /// `via_guess` is true when the label matched the unsure guess.
    /// A known decision match leaves it false.
    pub fn add_choice_compared(&mut self, matched: bool, via_guess: bool) {
        self.choice_n += 1;
        if matched {
            self.choice_hit += 1;
        }
        if via_guess {
            self.choice_guess += 1;
        } else {
            self.choice_known += 1;
        }
    }

    /// Squared error on a 0 to 1 scale. `max` is the score criterion width.
    pub fn add_score(&mut self, predicted: f64, label: f64, max: f64) {
        let scale = if max <= 0.0 { 1.0 } else { max };
        let err = ((predicted - label) / scale).clamp(-1.0, 1.0);
        self.brier_sum += err * err;
        self.brier_n += 1;
    }

    pub fn brier(&self) -> Option<f64> {
        if self.brier_n == 0 {
            None
        } else {
            Some(self.brier_sum / self.brier_n as f64)
        }
    }

    pub fn choice_accuracy(&self) -> Option<f64> {
        if self.choice_n == 0 {
            None
        } else {
            Some(self.choice_hit as f64 / self.choice_n as f64)
        }
    }

    /// How many choice labels were compared to a known decision, then to a guess.
    pub fn choice_compared(&self) -> (u64, u64) {
        (self.choice_known, self.choice_guess)
    }

    pub fn bins(&self) -> impl Iterator<Item = (usize, u64, f64)> + '_ {
        self.bins.iter().enumerate().filter_map(|(index, count)| {
            if *count == 0 {
                None
            } else {
                Some((index, *count, self.bin_hits[index] as f64 / *count as f64))
            }
        })
    }

    pub fn is_empty(&self) -> bool {
        self.brier_n == 0 && self.choice_n == 0
    }
}

#[cfg(test)]
mod tests {
    use super::Scorecard;

    #[test]
    fn a_confident_wrong_noul_scores_worse_than_a_low_one() {
        let mut wrong = Scorecard::default();
        wrong.add_noul(0.9, false);
        let mut closer = Scorecard::default();
        closer.add_noul(0.1, false);
        assert!(wrong.brier().unwrap() > closer.brier().unwrap());
    }

    #[test]
    fn choice_accuracy_counts_matches() {
        let mut card = Scorecard::default();
        card.add_choice(true);
        card.add_choice(false);
        assert_eq!(card.choice_accuracy().unwrap(), 0.5);
    }

    #[test]
    fn noul_probability_one_lands_in_the_last_bin() {
        let mut card = Scorecard::default();
        assert!(card.is_empty());
        assert_eq!(card.brier(), None);
        assert_eq!(card.choice_accuracy(), None);
        assert_eq!(card.bins().count(), 0);

        card.add_noul(0.5, false);
        assert_eq!(card.brier(), Some(0.25));
        assert_eq!(card.bins().collect::<Vec<_>>(), vec![(2, 1, 0.0)]);

        card.add_noul(1.0, true);
        assert_eq!(card.brier(), Some(0.125));
        assert_eq!(
            card.bins().collect::<Vec<_>>(),
            vec![(2, 1, 0.0), (4, 1, 1.0)]
        );
    }

    #[test]
    fn score_width_clamps_the_squared_error() {
        let mut wide = Scorecard::default();
        wide.add_score(2.0, 0.0, 1.0);
        assert_eq!(wide.brier(), Some(1.0));

        let mut zero_width = Scorecard::default();
        zero_width.add_score(0.5, 0.0, 0.0);
        assert_eq!(zero_width.brier(), Some(0.25));
    }

    #[test]
    fn a_guess_match_is_not_counted_as_known() {
        let mut card = Scorecard::default();
        card.add_choice_compared(false, true);
        assert_eq!(card.choice_accuracy(), Some(0.0));
        assert_eq!(card.choice_compared(), (0, 1));
    }
}
