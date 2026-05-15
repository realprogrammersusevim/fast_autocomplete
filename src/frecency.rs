use std::collections::HashMap;
use std::path::Path;
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};

/// Decay time-constant. Score is multiplied by exp(-Δt / TAU) between events.
/// τ = 7 days → an unused entry retains ~37% of its score after a week,
/// ~13% after two weeks, ~0.5% after a month.
const TAU: Duration = Duration::from_secs(7 * 86_400);

/// Compaction thresholds: an entry is dropped on save when its decayed
/// score is below this AND it has been idle for longer than the idle cutoff.
/// Both must hold so brand-new low-score entries aren't pruned immediately.
const PRUNE_SCORE: f64 = 0.05;
const PRUNE_IDLE: Duration = Duration::from_secs(30 * 86_400);

#[derive(Serialize, Deserialize)]
pub struct FrecencyEntry {
    score: f64,
    last_used: SystemTime,
}

#[derive(Default, Serialize, Deserialize)]
pub struct FrecencyStore {
    entries: HashMap<String, FrecencyEntry>,
}

fn decay(score: f64, age: Duration) -> f64 {
    score * (-(age.as_secs_f64()) / TAU.as_secs_f64()).exp()
}

impl FrecencyStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record(&mut self, completion: &str) {
        let now = SystemTime::now();
        let entry = self
            .entries
            .entry(completion.to_string())
            .or_insert(FrecencyEntry {
                score: 0.0,
                last_used: now,
            });
        let age = now.duration_since(entry.last_used).unwrap_or_default();
        entry.score = decay(entry.score, age) + 1.0;
        entry.last_used = now;
    }

    pub fn score(&self, completion: &str) -> f64 {
        let Some(entry) = self.entries.get(completion) else {
            return 0.0;
        };
        let age = SystemTime::now()
            .duration_since(entry.last_used)
            .unwrap_or_default();
        decay(entry.score, age)
    }

    /// Drop entries whose decayed score has fallen below `PRUNE_SCORE` and
    /// have been idle for longer than `PRUNE_IDLE`. Called on every save so
    /// the on-disk file (and the in-memory map after load) stays bounded.
    fn compact(&mut self) {
        let now = SystemTime::now();
        self.entries.retain(|_, entry| {
            let age = now.duration_since(entry.last_used).unwrap_or_default();
            !(age > PRUNE_IDLE && decay(entry.score, age) < PRUNE_SCORE)
        });
    }

    /// Load from disk. Returns an empty store if the file does not exist or is
    /// unreadable / corrupt — the on-disk format is best-effort, not load-bearing.
    pub fn load(path: &Path) -> Self {
        let Ok(bytes) = std::fs::read(path) else {
            return Self::new();
        };
        bincode::deserialize(&bytes).unwrap_or_else(|e| {
            log::warn!("frecency: failed to decode {}: {e}", path.display());
            Self::new()
        })
    }

    pub fn save(&mut self, path: &Path) -> anyhow::Result<()> {
        self.compact();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let bytes = bincode::serialize(self)?;
        // Atomic-ish: write to temp and rename so a crash mid-write doesn't truncate the file.
        let tmp = path.with_extension("bin.tmp");
        std::fs::write(&tmp, bytes)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn insert_entry(&mut self, completion: &str, score: f64, age: std::time::Duration) {
        self.entries.insert(
            completion.to_string(),
            FrecencyEntry {
                score,
                last_used: SystemTime::now().checked_sub(age).unwrap(),
            },
        );
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    #[allow(clippy::float_cmp)]
    fn test_unrecorded_scores_zero() {
        let store = FrecencyStore::new();
        assert_eq!(store.score("anything"), 0.0);
    }

    #[test]
    fn test_single_record_positive_score() {
        let mut store = FrecencyStore::new();
        store.record("git");
        let s = store.score("git");
        assert!(
            (0.99..=1.01).contains(&s),
            "expected score ≈ 1.0, got {s}"
        );
    }

    #[test]
    fn test_multiple_records_increase_score() {
        let mut store = FrecencyStore::new();
        store.record("git");
        store.record("git");
        store.record("git");
        let s = store.score("git");
        // Three back-to-back records (negligible decay) ≈ 3.0.
        assert!((2.95..=3.05).contains(&s), "expected score ≈ 3.0, got {s}");
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
    fn test_decay_recent_entry_near_full() {
        let mut store = FrecencyStore::new();
        store.insert_entry("x", 1.0, Duration::from_secs(100));
        let s = store.score("x");
        assert!((0.99..=1.0).contains(&s), "expected ≈ 1.0, got {s}");
    }

    #[test]
    fn test_decay_after_one_tau() {
        let mut store = FrecencyStore::new();
        store.insert_entry("x", 1.0, TAU);
        let s = store.score("x");
        // exp(-1) ≈ 0.3679
        assert!((0.36..=0.38).contains(&s), "expected ≈ 0.368, got {s}");
    }

    #[test]
    fn test_decay_after_two_tau() {
        let mut store = FrecencyStore::new();
        store.insert_entry("x", 1.0, TAU * 2);
        let s = store.score("x");
        // exp(-2) ≈ 0.1353
        assert!((0.12..=0.15).contains(&s), "expected ≈ 0.135, got {s}");
    }

    #[test]
    fn test_record_decays_prior_score() {
        let mut store = FrecencyStore::new();
        // Seed: score 10.0, last used one τ ago → decayed to ~3.68 at record time.
        store.insert_entry("x", 10.0, TAU);
        store.record("x");
        let s = store.score("x");
        // 10 * e^-1 + 1 ≈ 4.68
        assert!((4.6..=4.8).contains(&s), "expected ≈ 4.68, got {s}");
    }

    #[test]
    fn test_compact_prunes_stale_low_score_entries() {
        let mut store = FrecencyStore::new();
        // Idle 31d with score 1.0 → decayed ≈ exp(-31/7) ≈ 0.012 → pruned.
        store.insert_entry("stale", 1.0, Duration::from_secs(31 * 86_400));
        // Idle 31d but score 100 → decayed ≈ 1.24 → kept.
        store.insert_entry("warm", 100.0, Duration::from_secs(31 * 86_400));
        // Idle 1d, low score → kept (under idle cutoff).
        store.insert_entry("fresh", 0.01, Duration::from_secs(86_400));
        store.compact();
        assert_eq!(store.score("stale"), 0.0);
        assert!(store.score("warm") > 0.0);
        assert!(store.score("fresh") > 0.0);
        assert_eq!(store.len(), 2);
    }

    #[test]
    fn test_roundtrip_save_load() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("frecency.bin");
        let mut store = FrecencyStore::new();
        store.record("git");
        store.record("git");
        store.record("cargo");
        store.save(&path).unwrap();
        let loaded = FrecencyStore::load(&path);
        assert!(loaded.score("git") > loaded.score("cargo"));
        assert!(loaded.score("cargo") > 0.0);
    }

    #[test]
    fn test_save_compacts_in_place() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("frecency.bin");
        let mut store = FrecencyStore::new();
        store.insert_entry("stale", 1.0, Duration::from_secs(31 * 86_400));
        store.record("active");
        assert_eq!(store.len(), 2);
        store.save(&path).unwrap();
        assert_eq!(store.len(), 1);
        let loaded = FrecencyStore::load(&path);
        assert_eq!(loaded.score("stale"), 0.0);
        assert!(loaded.score("active") > 0.0);
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn test_load_missing_file_returns_empty() {
        let store = FrecencyStore::load(Path::new("/nonexistent/path/frecency.bin"));
        assert_eq!(store.score("anything"), 0.0);
    }
}
