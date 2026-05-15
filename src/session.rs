use std::hash::{Hash, Hasher};
use std::time::Instant;

pub struct SessionState {
    last_hash: u64,
    pub last_seen: Instant,
}

impl Default for SessionState {
    fn default() -> Self {
        Self {
            last_hash: 0,
            last_seen: Instant::now(),
        }
    }
}

impl SessionState {
    /// Returns true if the completion list is identical to the last response sent to this session.
    /// Updates the stored hash when the list has changed.
    pub fn is_duplicate(&mut self, completions: &[String]) -> bool {
        let mut hasher = ahash::AHasher::default();
        completions.hash(&mut hasher);
        let h = hasher.finish();
        if h == self.last_hash {
            true
        } else {
            self.last_hash = h;
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_first_call_not_duplicate() {
        let mut s = SessionState::default();
        assert!(!s.is_duplicate(&["a".to_string()]));
    }

    #[test]
    fn test_same_list_twice_is_duplicate() {
        let mut s = SessionState::default();
        let list = vec!["a".to_string(), "b".to_string()];
        assert!(!s.is_duplicate(&list));
        assert!(s.is_duplicate(&list));
    }

    #[test]
    fn test_different_list_resets() {
        let mut s = SessionState::default();
        assert!(!s.is_duplicate(&["a".to_string(), "b".to_string()]));
        assert!(s.is_duplicate(&["a".to_string(), "b".to_string()]));
        assert!(!s.is_duplicate(&["a".to_string(), "c".to_string()]));
    }

    #[test]
    fn test_empty_list_dedup() {
        let mut s = SessionState::default();
        assert!(!s.is_duplicate(&[]));
        assert!(s.is_duplicate(&[]));
    }

    #[test]
    fn test_single_item() {
        let mut s = SessionState::default();
        assert!(!s.is_duplicate(&["solo".to_string()]));
        assert!(s.is_duplicate(&["solo".to_string()]));
    }

    #[test]
    fn test_order_matters() {
        let mut s = SessionState::default();
        assert!(!s.is_duplicate(&["a".to_string(), "b".to_string()]));
        // Different order → different hash → not a duplicate
        assert!(!s.is_duplicate(&["b".to_string(), "a".to_string()]));
    }
}
