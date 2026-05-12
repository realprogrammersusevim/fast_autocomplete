use crate::frecency::FrecencyStore;

/// Merge static cache completions and file completions, prefix-filter, deduplicate,
/// and sort by frecency then type (non-flags before flags) then alphabetically.
pub fn rank_completions(
    static_items: Vec<String>,
    file_items: Vec<String>,
    current_word: &str,
    frecency: &FrecencyStore,
) -> Vec<String> {
    let mut all: Vec<String> = static_items
        .into_iter()
        .chain(file_items)
        .filter(|s| s.starts_with(current_word))
        .collect();

    all.sort();
    all.dedup();

    let mut scored: Vec<(f64, bool, String)> = all
        .into_iter()
        .map(|s| {
            let score = frecency.score(&s);
            let is_flag = s.starts_with('-');
            (score, is_flag, s)
        })
        .collect();

    // High frecency first, then non-flags before flags, then alphabetical.
    scored.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.1.cmp(&b.1))
            .then(a.2.cmp(&b.2))
    });

    scored.into_iter().take(200).map(|(_, _, s)| s).collect()
}
