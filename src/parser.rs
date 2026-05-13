pub struct ParsedBuffer {
    /// Words used for tree lookup (excludes the partial current word).
    pub lookup_words: Vec<String>,
    /// The partial word at the cursor being completed (may be empty).
    pub current_word: String,
}

pub fn parse_buffer(buffer: &str, cursor: usize) -> ParsedBuffer {
    let relevant = &buffer[..cursor.min(buffer.len())];
    let words = split_shell_words(relevant);
    let cursor_after_space = relevant.chars().last().is_some_and(char::is_whitespace);

    if cursor_after_space || words.is_empty() {
        ParsedBuffer {
            lookup_words: words,
            current_word: String::new(),
        }
    } else {
        let current_word = words.last().cloned().unwrap_or_default();
        let lookup_words = words[..words.len() - 1].to_vec();
        ParsedBuffer {
            lookup_words,
            current_word,
        }
    }
}

/// Split a shell command line into words, respecting single/double quotes and backslash escapes.
pub fn split_shell_words(s: &str) -> Vec<String> {
    enum State {
        Normal,
        InSingle,
        InDouble,
        Backslash,
        DoubleBackslash,
    }

    let mut words = Vec::new();
    let mut current = String::new();
    let mut state = State::Normal;

    for c in s.chars() {
        match state {
            State::Normal => match c {
                '\'' => state = State::InSingle,
                '"' => state = State::InDouble,
                '\\' => state = State::Backslash,
                ' ' | '\t' => {
                    if !current.is_empty() {
                        words.push(current.clone());
                        current.clear();
                    }
                }
                _ => current.push(c),
            },
            State::InSingle => match c {
                '\'' => state = State::Normal,
                _ => current.push(c),
            },
            State::InDouble => match c {
                '"' => state = State::Normal,
                '\\' => state = State::DoubleBackslash,
                _ => current.push(c),
            },
            State::Backslash => {
                current.push(c);
                state = State::Normal;
            }
            State::DoubleBackslash => {
                current.push(c);
                state = State::InDouble;
            }
        }
    }

    if !current.is_empty() {
        words.push(current);
    }

    words
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_split() {
        assert_eq!(split_shell_words("git add foo"), vec!["git", "add", "foo"]);
    }

    #[test]
    fn test_quoted() {
        assert_eq!(
            split_shell_words("echo 'hello world'"),
            vec!["echo", "hello world"]
        );
    }

    #[test]
    fn test_parse_partial_word() {
        let p = parse_buffer("git add fo", 10);
        assert_eq!(p.lookup_words, vec!["git", "add"]);
        assert_eq!(p.current_word, "fo");
    }

    #[test]
    fn test_parse_after_space() {
        let p = parse_buffer("git add ", 8);
        assert_eq!(p.lookup_words, vec!["git", "add"]);
        assert_eq!(p.current_word, "");
    }

    #[test]
    fn test_empty_string() {
        assert_eq!(split_shell_words(""), Vec::<String>::new());
    }

    #[test]
    fn test_single_word_no_space() {
        assert_eq!(split_shell_words("git"), vec!["git"]);
    }

    #[test]
    fn test_tab_separator() {
        assert_eq!(
            split_shell_words("git\tadd\tfoo"),
            vec!["git", "add", "foo"]
        );
    }

    #[test]
    fn test_double_quote_basic() {
        assert_eq!(
            split_shell_words(r#"echo "hello world""#),
            vec!["echo", "hello world"]
        );
    }

    #[test]
    fn test_double_quote_backslash_escape() {
        // Exercises the DoubleBackslash state: \" inside double quotes → literal "
        assert_eq!(
            split_shell_words(r#"echo "hel\"lo""#),
            vec!["echo", r#"hel"lo"#]
        );
    }

    #[test]
    fn test_backslash_space() {
        // Backslash before space → literal space, stays in same word
        assert_eq!(
            split_shell_words(r"echo hello\ world"),
            vec!["echo", "hello world"]
        );
    }

    #[test]
    fn test_mixed_quotes() {
        assert_eq!(
            split_shell_words(r#"git commit -m 'fix: "bug"'"#),
            vec!["git", "commit", "-m", r#"fix: "bug""#]
        );
    }

    #[test]
    fn test_parse_buffer_cursor_zero() {
        let p = parse_buffer("git add foo", 0);
        assert!(p.lookup_words.is_empty());
        assert_eq!(p.current_word, "");
    }

    #[test]
    fn test_parse_buffer_cursor_beyond_end() {
        // cursor=999 is clamped to buffer length (3)
        let p = parse_buffer("git", 999);
        assert!(p.lookup_words.is_empty());
        assert_eq!(p.current_word, "git");
    }

    #[test]
    fn test_parse_buffer_empty_input() {
        let p = parse_buffer("", 0);
        assert!(p.lookup_words.is_empty());
        assert_eq!(p.current_word, "");
    }
}
