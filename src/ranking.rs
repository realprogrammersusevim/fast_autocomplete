use std::collections::HashMap;
use std::sync::LazyLock;

use fuzzy_matcher::FuzzyMatcher;
use fuzzy_matcher::skim::SkimMatcherV2;

static MATCHER: LazyLock<SkimMatcherV2> =
    LazyLock::new(|| SkimMatcherV2::default().smart_case());

/// Merge static cache completions and file completions, fuzzy-filter, deduplicate,
/// and sort by combined fuzzy+frecency score, then type (non-flags before flags), then alpha.
/// Flags (items starting with `-`) are only included when `current_word` starts with `-`.
pub fn rank_completions(
    static_items: Vec<String>,
    file_items: Vec<String>,
    current_word: &str,
    frecency_scores: &HashMap<String, f64>,
) -> Vec<String> {
    let typing_flag = current_word.starts_with('-');

    let mut all: Vec<String> = static_items
        .into_iter()
        .chain(file_items)
        .filter(|s| typing_flag || !s.starts_with('-'))
        .collect();

    all.sort();
    all.dedup();

    let freq = |s: &str| frecency_scores.get(s).copied().unwrap_or(0.0);

    let mut scored: Vec<(f64, bool, String)> = if current_word.is_empty() {
        all.into_iter()
            .map(|s| {
                let score = freq(&s);
                let is_flag = s.starts_with('-');
                (score, is_flag, s)
            })
            .collect()
    } else {
        all.into_iter()
            .filter_map(|s| {
                MATCHER.fuzzy_match(&s, current_word).map(|fuzzy_score| {
                    // Frecency dominates; fuzzy score breaks ties among zero-frecency items.
                    let combined = freq(&s) * 100.0 + fuzzy_score as f64;
                    let is_flag = s.starts_with('-');
                    (combined, is_flag, s)
                })
            })
            .collect()
    };

    // High score first, then non-flags before flags, then alphabetical.
    scored.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.1.cmp(&b.1))
            .then(a.2.cmp(&b.2))
    });

    scored.into_iter().take(200).map(|(_, _, s)| s).collect()
}
