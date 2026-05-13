use std::collections::HashMap;
use std::time::{Duration, Instant};

pub struct FrecencyEntry {
    count: u32,
    last_used: Instant,
}

pub struct FrecencyStore {
    entries: HashMap<String, FrecencyEntry>,
}

impl FrecencyStore {
    pub fn new() -> Self {
        FrecencyStore {
            entries: HashMap::new(),
        }
    }

    pub fn record(&mut self, completion: &str) {
        let entry = self
            .entries
            .entry(completion.to_string())
            .or_insert(FrecencyEntry {
                count: 0,
                last_used: Instant::now(),
            });
        entry.count += 1;
        entry.last_used = Instant::now();
    }

    pub fn score(&self, completion: &str) -> f64 {
        let Some(entry) = self.entries.get(completion) else {
            return 0.0;
        };
        let age = entry.last_used.elapsed();
        let weight = if age < Duration::from_secs(3_600) {
            1.0
        } else if age < Duration::from_secs(86_400) {
            0.8
        } else if age < Duration::from_secs(604_800) {
            0.6
        } else {
            0.4
        };
        entry.count as f64 * weight
    }

    #[cfg(test)]
    pub(crate) fn insert_entry(&mut self, completion: &str, count: u32, age: std::time::Duration) {
        self.entries.insert(
            completion.to_string(),
            FrecencyEntry { count, last_used: std::time::Instant::now() - age },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn test_unrecorded_scores_zero() {
        let store = FrecencyStore::new();
        assert_eq!(store.score("anything"), 0.0);
    }

    #[test]
    fn test_single_record_positive_score() {
        let mut store = FrecencyStore::new();
        store.record("git");
        let s = store.score("git");
        assert!(s >= 1.0 && s < 2.0, "expected score in [1.0, 2.0), got {}", s);
    }

    #[test]
    fn test_multiple_records_increase_score() {
        let mut store = FrecencyStore::new();
        store.record("git");
        store.record("git");
        store.record("git");
        let s = store.score("git");
        assert!(s >= 2.9 && s <= 3.1, "expected score ≈ 3.0, got {}", s);
    }

    #[test]
    fn test_score_ordering_favors_frequency() {
        let mut store = FrecencyStore::new();
        store.record("git");
        store.record("git");
        store.record("git");
        store.record("cargo");
        assert!(store.score("git") > store.score("cargo"));
    }

    #[test]
    fn test_weight_tier_under_1_hour() {
        let mut store = FrecencyStore::new();
        store.insert_entry("x", 1, Duration::from_secs(100));
        assert_eq!(store.score("x"), 1.0);
    }

    #[test]
    fn test_weight_tier_1_to_24_hours() {
        let mut store = FrecencyStore::new();
        store.insert_entry("x", 1, Duration::from_secs(7_200));
        assert_eq!(store.score("x"), 0.8);
    }

    #[test]
    fn test_weight_tier_1_to_7_days() {
        let mut store = FrecencyStore::new();
        store.insert_entry("x", 1, Duration::from_secs(172_800));
        assert_eq!(store.score("x"), 0.6);
    }

    #[test]
    fn test_weight_tier_over_7_days() {
        let mut store = FrecencyStore::new();
        store.insert_entry("x", 1, Duration::from_secs(1_209_600));
        assert_eq!(store.score("x"), 0.4);
    }
}
