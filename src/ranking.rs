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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_inputs() {
        let result = rank_completions(vec![], vec![], "", &HashMap::new());
        assert!(result.is_empty());
    }

    #[test]
    fn test_flags_hidden_when_not_typing_flag() {
        let result = rank_completions(
            vec!["--verbose".to_string(), "subcommand".to_string()],
            vec![],
            "",
            &HashMap::new(),
        );
        assert!(result.contains(&"subcommand".to_string()));
        assert!(!result.contains(&"--verbose".to_string()));
    }

    #[test]
    fn test_flags_shown_when_typing_flag() {
        let result = rank_completions(
            vec!["--verbose".to_string(), "--help".to_string(), "subcommand".to_string()],
            vec![],
            "--",
            &HashMap::new(),
        );
        assert!(result.contains(&"--verbose".to_string()));
        assert!(result.contains(&"--help".to_string()));
        // "subcommand" has no '-' chars, so fuzzy match against "--" drops it
        assert!(!result.contains(&"subcommand".to_string()));
    }

    #[test]
    fn test_frecency_dominates_fuzzy() {
        let mut scores = HashMap::new();
        scores.insert("apricot".to_string(), 100.0);
        let result = rank_completions(
            vec!["apple".to_string(), "apricot".to_string()],
            vec![],
            "ap",
            &scores,
        );
        assert!(!result.is_empty());
        assert_eq!(result[0], "apricot");
    }

    #[test]
    fn test_dedup_static_file_overlap() {
        let result = rank_completions(
            vec!["foo".to_string(), "bar".to_string()],
            vec!["foo".to_string(), "baz".to_string()],
            "",
            &HashMap::new(),
        );
        assert_eq!(result.iter().filter(|s| s.as_str() == "foo").count(), 1);
    }

    #[test]
    fn test_cap_at_200() {
        let items: Vec<String> = (0..300).map(|i| format!("item_{:03}", i)).collect();
        let result = rank_completions(items, vec![], "", &HashMap::new());
        assert_eq!(result.len(), 200);
    }

    #[test]
    fn test_fuzzy_filter_drops_non_matches() {
        // "xyzzy" contains 'x' which is absent from all candidates
        let result = rank_completions(
            vec!["add".to_string(), "commit".to_string(), "checkout".to_string()],
            vec![],
            "xyzzy",
            &HashMap::new(),
        );
        assert!(result.is_empty());
    }

    #[test]
    fn test_alpha_sort_as_tiebreaker() {
        let result = rank_completions(
            vec!["zebra".to_string(), "apple".to_string(), "mango".to_string()],
            vec![],
            "",
            &HashMap::new(),
        );
        assert_eq!(result, vec!["apple", "mango", "zebra"]);
    }

    #[test]
    fn test_file_items_included_in_output() {
        let result = rank_completions(
            vec!["add".to_string()],
            vec!["./README.md".to_string()],
            "",
            &HashMap::new(),
        );
        assert!(result.contains(&"add".to_string()));
        assert!(result.contains(&"./README.md".to_string()));
    }
}
