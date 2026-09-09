use std::io::{self, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

// Exit codes: 0=results, 1=daemon failure, 2=unchanged, 3=empty.
pub fn run_complete(socket_path: &Path) -> ! {
    let mut payload = String::new();
    let _ = io::stdin().read_to_string(&mut payload);

    let Some(resp) = query(socket_path, &payload) else {
        std::process::exit(1);
    };

    if resp.unchanged {
        std::process::exit(2);
    }
    let completions = resp.completions;
    if completions.is_empty() {
        std::process::exit(3);
    }
    for c in &completions {
        println!("{c}");
    }
    std::process::exit(0);
}

pub fn run_display(socket_path: &Path, columns: usize) -> ! {
    let mut payload = String::new();
    let _ = io::stdin().read_to_string(&mut payload);

    let Some(resp) = query(socket_path, &payload) else {
        std::process::exit(1);
    };

    if resp.unchanged {
        std::process::exit(2);
    }
    let completions = resp.completions;
    if completions.is_empty() {
        std::process::exit(3);
    }
    print!("{}", format_columns(&completions, columns));
    std::process::exit(0);
}

pub fn run_record(socket_path: &Path, value: &str) -> ! {
    if !value.is_empty() {
        let payload = format!("RECORD={value}\n\n");
        if let Ok(mut stream) = UnixStream::connect(socket_path) {
            let _ = stream.set_write_timeout(Some(Duration::from_millis(500)));
            if stream.write_all(payload.as_bytes()).is_ok() {
                // Shutdown write then drain: guarantees delivery before exit.
                let _ = stream.shutdown(std::net::Shutdown::Write);
                let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
                let mut sink = [0u8; 64];
                while let Ok(n) = stream.read(&mut sink) {
                    if n == 0 {
                        break;
                    }
                }
            }
        }
    }
    std::process::exit(0);
}

#[derive(serde::Deserialize)]
struct DaemonResponse {
    #[serde(default)]
    completions: Vec<String>,
    #[serde(default)]
    unchanged: bool,
}

fn query(socket_path: &Path, payload: &str) -> Option<DaemonResponse> {
    let mut stream = UnixStream::connect(socket_path).ok()?;
    stream
        .set_write_timeout(Some(Duration::from_secs(1)))
        .ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(1))).ok()?;
    stream.write_all(payload.as_bytes()).ok()?;
    let _ = stream.shutdown(std::net::Shutdown::Write);
    let mut response = String::new();
    stream.read_to_string(&mut response).ok()?;
    serde_json::from_str(response.trim()).ok()
}

fn format_columns(completions: &[String], term_width: usize) -> String {
    const MAX_ROWS: usize = 8;
    const MAX_SHOWN_INIT: usize = 40;

    // Never fill the final column: a row exactly $COLUMNS wide soft-wraps into
    // a second terminal row that zsh's `zle -M` line accounting doesn't know
    // about, so each redraw reprints the prompt one row higher and eats the
    // scrollback above it.
    let usable = term_width.saturating_sub(1).max(1);

    // Truncate over-wide items (long paths) so a single entry can't wrap.
    let items: Vec<String> = completions
        .iter()
        .map(|c| {
            if c.chars().count() > usable {
                c.chars().take(usable.saturating_sub(1)).collect::<String>() + "\u{2026}"
            } else {
                c.clone()
            }
        })
        .collect();

    let max_len = items.iter().map(|c| c.chars().count()).max().unwrap_or(0);
    let col_width = (max_len + 2).clamp(1, usable);
    let num_cols = (usable / col_width).max(1);

    // Reserve the last row for the overflow notice when there is one.
    let cap = num_cols * MAX_ROWS;
    let (max_shown, overflow) = if items.len() > cap.min(MAX_SHOWN_INIT) {
        ((num_cols * (MAX_ROWS - 1)).min(MAX_SHOWN_INIT), true)
    } else {
        (items.len(), false)
    };
    let shown = &items[..max_shown.min(items.len())];

    let mut output = String::new();
    for (i, c) in shown.iter().enumerate() {
        let last_in_row = (i + 1) % num_cols == 0 || i + 1 == shown.len();
        output.push_str(c);
        if last_in_row {
            output.push('\n');
        } else {
            for _ in c.chars().count()..col_width {
                output.push(' ');
            }
        }
    }

    if output.ends_with('\n') {
        output.pop();
    }

    if overflow {
        use std::fmt::Write;
        let more = completions.len() - shown.len();
        let notice = format!("  \u{2026} ({more} more, press Tab to browse)");
        let notice: String = notice.chars().take(usable).collect();
        write!(output, "\n{notice}").unwrap();
    }

    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_columns_two_cols() {
        let items: Vec<String> = vec!["add".into(), "commit".into(), "push".into()];
        // max_len=6 (commit), col_width=8; width=20 → num_cols=2
        let out = format_columns(&items, 20);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].starts_with("add"));
        assert!(lines[1].starts_with("push"));
    }

    #[test]
    fn test_format_columns_overflow_notice() {
        // 41 items → overflow with max_shown=40
        let items: Vec<String> = (0..41).map(|i| format!("item{i}")).collect();
        let out = format_columns(&items, 200);
        assert!(out.contains("more, press Tab to browse"));
    }

    #[test]
    fn test_format_columns_never_fills_terminal_width() {
        // 80 / 8 == 10 exactly: the old code emitted rows exactly 80 wide,
        // which soft-wrap and walk the prompt up the screen.
        let items: Vec<String> = (0..30).map(|i| format!("item{i:02}")).collect();
        let out = format_columns(&items, 80);
        for line in out.lines() {
            assert!(line.chars().count() < 80, "line too wide: {line:?}");
        }
    }

    #[test]
    fn test_format_columns_caps_total_lines() {
        let items: Vec<String> = (0..500).map(|i| format!("item{i}")).collect();
        let out = format_columns(&items, 80);
        assert!(out.lines().count() <= 8, "{} lines", out.lines().count());
        assert!(out.contains("more, press Tab to browse"));
    }

    #[test]
    fn test_format_columns_truncates_overlong_item() {
        let items: Vec<String> = vec!["x".repeat(200)];
        let out = format_columns(&items, 40);
        assert_eq!(out.lines().count(), 1);
        assert!(out.chars().count() < 40);
        assert!(out.ends_with('\u{2026}'));
    }

    #[test]
    fn test_format_columns_no_trailing_newline() {
        let items: Vec<String> = vec!["a".into(), "b".into()];
        let out = format_columns(&items, 80);
        assert!(!out.ends_with('\n'));
    }

    #[test]
    fn test_daemon_response_deserialize() {
        let r: DaemonResponse =
            serde_json::from_str(r#"{"completions":["add","commit"],"unchanged":false}"#).unwrap();
        assert_eq!(r.completions, vec!["add", "commit"]);
        assert!(!r.unchanged);
    }

    #[test]
    fn test_daemon_response_unchanged() {
        let r: DaemonResponse = serde_json::from_str(r#"{"unchanged":true}"#).unwrap();
        assert!(r.unchanged);
        assert!(r.completions.is_empty());
    }
}
