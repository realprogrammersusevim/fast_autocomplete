pub struct ParsedBuffer {
    /// Words used for tree lookup (excludes the partial current word).
    pub lookup_words: Vec<String>,
    /// The partial word at the cursor being completed (may be empty).
    pub current_word: String,
}

pub fn parse_buffer(buffer: &str, cursor: usize) -> ParsedBuffer {
    let relevant = &buffer[..cursor.min(buffer.len())];
    let words = split_shell_words(relevant);
    let cursor_after_space = relevant
        .chars()
        .last()
        .map(|c| c.is_whitespace())
        .unwrap_or(false);

    if cursor_after_space || words.is_empty() {
        ParsedBuffer {
            lookup_words: words,
            current_word: String::new(),
        }
    } else {
        let current_word = words.last().cloned().unwrap_or_default();
        let lookup_words = words[..words.len() - 1].to_vec();
        ParsedBuffer { lookup_words, current_word }
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
        assert_eq!(split_shell_words("echo 'hello world'"), vec!["echo", "hello world"]);
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
}
