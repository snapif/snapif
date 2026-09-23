//! Labeled-score totals for `snapif calibrate`.

#[derive(Debug, Default, Clone)]
pub struct Scorecard {
    brier_sum: f64,
    brier_n: u64,
    bins: [u64; 5],
    bin_hits: [u64; 5],
    choice_hit: u64,
    choice_n: u64,
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
        self.choice_n += 1;
        if matched {
            self.choice_hit += 1;
        }
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
}
