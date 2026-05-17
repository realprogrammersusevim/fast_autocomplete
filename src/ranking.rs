use std::sync::LazyLock;

use fuzzy_matcher::FuzzyMatcher;
use fuzzy_matcher::skim::SkimMatcherV2;

static MATCHER: LazyLock<SkimMatcherV2> = LazyLock::new(|| SkimMatcherV2::default().smart_case());

pub fn rank_completions(
    static_items: &[String],
    file_items: &[String],
    current_word: &str,
    freq: impl Fn(&str) -> f64,
) -> Vec<String> {
    let typing_flag = current_word.starts_with('-');

    let mut all: Vec<&str> = static_items
        .iter()
        .map(String::as_str)
        .chain(file_items.iter().map(String::as_str))
        .filter(|s| typing_flag || !s.starts_with('-'))
        .collect();

    all.sort_unstable();
    all.dedup();

    let mut scored: Vec<(f64, bool, &str)> = if current_word.is_empty() {
        all.into_iter()
            .map(|s| {
                let score = freq(s);
                let is_flag = s.starts_with('-');
                (score, is_flag, s)
            })
            .collect()
    } else {
        all.into_iter()
            .filter_map(|s| {
                MATCHER.fuzzy_match(s, current_word).map(|fuzzy_score| {
                    // Frecency dominates; fuzzy score breaks ties among zero-frecency items.
                    let combined = freq(s).mul_add(100.0, fuzzy_score as f64);
                    let is_flag = s.starts_with('-');
                    (combined, is_flag, s)
                })
            })
            .collect()
    };

    scored.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.1.cmp(&b.1))
            .then(a.2.cmp(b.2))
    });

    scored
        .into_iter()
        .take(200)
        .map(|(_, _, s)| s.to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strs(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_string()).collect()
    }

    fn zero(_: &str) -> f64 {
        0.0
    }

    #[test]
    fn test_empty_inputs() {
        let result = rank_completions(&[], &[], "", zero);
        assert!(result.is_empty());
    }

    #[test]
    fn test_flags_hidden_when_not_typing_flag() {
        let result = rank_completions(&strs(&["--verbose", "subcommand"]), &[], "", zero);
        assert!(result.contains(&"subcommand".to_string()));
        assert!(!result.contains(&"--verbose".to_string()));
    }

    #[test]
    fn test_flags_shown_when_typing_flag() {
        let result = rank_completions(
            &strs(&["--verbose", "--help", "subcommand"]),
            &[],
            "--",
            zero,
        );
        assert!(result.contains(&"--verbose".to_string()));
        assert!(result.contains(&"--help".to_string()));
        // "subcommand" has no '-' chars, so fuzzy match against "--" drops it
        assert!(!result.contains(&"subcommand".to_string()));
    }

    #[test]
    fn test_frecency_dominates_fuzzy() {
        let result = rank_completions(&strs(&["apple", "apricot"]), &[], "ap", |s| {
            if s == "apricot" { 100.0 } else { 0.0 }
        });
        assert!(!result.is_empty());
        assert_eq!(result[0], "apricot");
    }

    #[test]
    fn test_dedup_static_file_overlap() {
        let result = rank_completions(&strs(&["foo", "bar"]), &strs(&["foo", "baz"]), "", zero);
        assert_eq!(result.iter().filter(|s| s.as_str() == "foo").count(), 1);
    }

    #[test]
    fn test_cap_at_200() {
        let items: Vec<String> = (0..300).map(|i| format!("item_{i:03}")).collect();
        let result = rank_completions(&items, &[], "", zero);
        assert_eq!(result.len(), 200);
    }

    #[test]
    fn test_fuzzy_filter_drops_non_matches() {
        // "xyzzy" contains 'x' which is absent from all candidates
        let result = rank_completions(&strs(&["add", "commit", "checkout"]), &[], "xyzzy", zero);
        assert!(result.is_empty());
    }

    #[test]
    fn test_alpha_sort_as_tiebreaker() {
        let result = rank_completions(&strs(&["zebra", "apple", "mango"]), &[], "", zero);
        assert_eq!(result, vec!["apple", "mango", "zebra"]);
    }

    #[test]
    fn test_file_items_included_in_output() {
        let result = rank_completions(&strs(&["add"]), &strs(&["./README.md"]), "", zero);
        assert!(result.contains(&"add".to_string()));
        assert!(result.contains(&"./README.md".to_string()));
    }
}
