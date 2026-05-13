use std::collections::HashMap;

pub struct Request {
    pub buffer: String,
    pub cursor: usize,
    pub cwd: String,
    pub session: u64,
}

impl Request {
    /// Parse a key=value block (lines terminated by `\n\n`).
    pub fn parse(input: &str) -> Self {
        let mut map: HashMap<&str, &str> = HashMap::new();
        for line in input.lines() {
            if let Some((k, v)) = line.split_once('=') {
                map.insert(k.trim(), v);
            }
        }
        Self {
            buffer: map.get("BUFFER").copied().unwrap_or("").to_string(),
            cursor: map.get("CURSOR").and_then(|s| s.parse().ok()).unwrap_or(0),
            cwd: map.get("CWD").copied().unwrap_or(".").to_string(),
            session: map.get("SESSION").and_then(|s| s.parse().ok()).unwrap_or(0),
        }
    }
}

#[derive(serde::Serialize)]
pub struct Response {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completions: Option<Vec<String>>,
    pub unchanged: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<&'static str>,
}

impl Response {
    pub const fn unchanged() -> Self {
        Self {
            completions: None,
            unchanged: true,
            error: None,
        }
    }

    pub const fn results(completions: Vec<String>) -> Self {
        Self {
            completions: Some(completions),
            unchanged: false,
            error: None,
        }
    }

    #[allow(dead_code)] // used in tests; not currently needed from binary code paths
    pub const fn error(msg: &'static str) -> Self {
        Self {
            completions: None,
            unchanged: false,
            error: Some(msg),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_full_request() {
        let input = "BUFFER=git add\nCURSOR=7\nCWD=/home/user\nSESSION=42\n";
        let req = Request::parse(input);
        assert_eq!(req.buffer, "git add");
        assert_eq!(req.cursor, 7);
        assert_eq!(req.cwd, "/home/user");
        assert_eq!(req.session, 42);
    }

    #[test]
    fn test_parse_missing_cursor_defaults_to_zero() {
        let req = Request::parse("BUFFER=git\nCWD=/tmp\nSESSION=1\n");
        assert_eq!(req.cursor, 0);
    }

    #[test]
    fn test_parse_invalid_cursor_defaults_to_zero() {
        let req = Request::parse("BUFFER=git\nCURSOR=notanumber\n");
        assert_eq!(req.cursor, 0);
    }

    #[test]
    fn test_parse_missing_cwd_defaults_to_dot() {
        let req = Request::parse("BUFFER=git\nCURSOR=0\nSESSION=1\n");
        assert_eq!(req.cwd, ".");
    }

    #[test]
    fn test_parse_missing_session_defaults_to_zero() {
        let req = Request::parse("BUFFER=git\nCURSOR=0\nCWD=/tmp\n");
        assert_eq!(req.session, 0);
    }

    #[test]
    fn test_parse_buffer_with_equals_sign() {
        // split_once('=') only splits on the first '=' — value may contain '='
        let req = Request::parse("BUFFER=git commit -m=fix\n");
        assert_eq!(req.buffer, "git commit -m=fix");
    }

    #[test]
    fn test_response_unchanged_serialization() {
        let json = serde_json::to_string(&Response::unchanged()).unwrap();
        assert!(json.contains("\"unchanged\":true"));
        assert!(!json.contains("completions"));
        assert!(!json.contains("error"));
    }

    #[test]
    fn test_response_results_serialization() {
        let json =
            serde_json::to_string(&Response::results(vec!["add".into(), "commit".into()])).unwrap();
        assert!(json.contains("\"completions\""));
        assert!(json.contains("\"add\""));
        assert!(json.contains("\"unchanged\":false"));
        assert!(!json.contains("\"error\""));
    }

    #[test]
    fn test_response_error_serialization() {
        let json = serde_json::to_string(&Response::error("parse_error")).unwrap();
        assert!(json.contains("\"error\":\"parse_error\""));
        assert!(json.contains("\"unchanged\":false"));
        assert!(!json.contains("\"completions\""));
    }

    #[test]
    fn test_response_results_empty_vec() {
        let json = serde_json::to_string(&Response::results(vec![])).unwrap();
        assert!(json.contains("\"completions\":[]"));
    }
}
