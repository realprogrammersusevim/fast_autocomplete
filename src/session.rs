use std::hash::{Hash, Hasher};

#[derive(Default)]
pub struct SessionState {
    last_hash: u64,
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
