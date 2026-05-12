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
        FrecencyStore { entries: HashMap::new() }
    }

    pub fn record(&mut self, completion: &str) {
        let entry = self.entries.entry(completion.to_string()).or_insert(FrecencyEntry {
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
}
