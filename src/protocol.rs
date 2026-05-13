use std::collections::HashMap;

pub struct Request {
    pub buffer: String,
    pub cursor: usize,
    pub cwd: String,
    pub session: u64,
}

impl Request {
    /// Parse a key=value block (lines terminated by \n\n).
    pub fn parse(input: &str) -> Option<Self> {
        let mut map: HashMap<&str, &str> = HashMap::new();
        for line in input.lines() {
            if let Some((k, v)) = line.split_once('=') {
                map.insert(k.trim(), v);
            }
        }
        Some(Request {
            buffer: map.get("BUFFER").copied().unwrap_or("").to_string(),
            cursor: map.get("CURSOR").and_then(|s| s.parse().ok()).unwrap_or(0),
            cwd: map.get("CWD").copied().unwrap_or(".").to_string(),
            session: map.get("SESSION").and_then(|s| s.parse().ok()).unwrap_or(0),
        })
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
    pub fn unchanged() -> Self {
        Response {
            completions: None,
            unchanged: true,
            error: None,
        }
    }

    pub fn results(completions: Vec<String>) -> Self {
        Response {
            completions: Some(completions),
            unchanged: false,
            error: None,
        }
    }

    pub fn error(msg: &'static str) -> Self {
        Response {
            completions: None,
            unchanged: false,
            error: Some(msg),
        }
    }
}
