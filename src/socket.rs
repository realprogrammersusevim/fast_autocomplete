use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

use crate::protocol::{Request, Response};
use crate::{SharedState, files, harvester, ranking};

pub async fn handle_connection(stream: UnixStream, state: Arc<SharedState>) {
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);
    let mut block = String::new();

    loop {
        let mut line = String::new();
        match reader.read_line(&mut line).await {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        if line.trim().is_empty() {
            break;
        }
        block.push_str(&line);
    }

    if block.is_empty() {
        return;
    }

    let response = process_request(&block, &state).await;
    if let Ok(json) = serde_json::to_string(&response) {
        let _ = writer.write_all(json.as_bytes()).await;
        let _ = writer.write_all(b"\n").await;
    }
}

fn ensure_harvested(cmd: &str, state: &Arc<SharedState>) {
    if state.harvested.contains_key(cmd) {
        return;
    }
    if !state.harvest_in_flight.insert(cmd.to_string()) {
        return;
    }

    let cmd_owned = cmd.to_string();
    let state_clone = Arc::clone(state);
    tokio::task::spawn_blocking(move || {
        if let Err(e) = harvester::harvest_command(&cmd_owned, &state_clone.tree) {
            log::warn!("harvest_command('{cmd_owned}') failed: {e}");
        }
        state_clone.harvested.insert(cmd_owned.clone(), ());
        state_clone.harvest_in_flight.remove(&cmd_owned);
    });
}

async fn process_request(raw: &str, state: &Arc<SharedState>) -> Response {
    let req = Request::parse(raw);

    if let Some(value) = req.record {
        // Plugin echoes the token from $BUFFER, which carries our escapes; strip
        // them so the frecency key matches what ranking scores against.
        let canonical = crate::parser::split_shell_words(&value)
            .into_iter()
            .next()
            .unwrap_or(value);
        state.frecency.write().await.record(&canonical);
        state.frecency_dirty.notify_one();
        return Response::unchanged();
    }

    let parsed = crate::parser::parse_buffer(&req.buffer, req.cursor);
    let cwd = std::path::Path::new(&req.cwd);

    if let Some(cmd) = parsed.lookup_words.first() {
        ensure_harvested(cmd, state);
    }

    let (flags, subs, wants_files, cmd_fallback) = if parsed.lookup_words.is_empty() {
        // Completing the command name itself — return all known command names.
        let cmds = match state.cmd_names.get() {
            Some(names) => names.clone(),
            None => state.tree.roots.iter().map(|e| e.key().clone()).collect(),
        };
        (None, None, true, Some(cmds))
    } else {
        let word_refs: Vec<&str> = parsed.lookup_words.iter().map(String::as_str).collect();
        match state.tree.lookup(&word_refs) {
            Some((f, s, w)) => (Some(f), Some(s), w, None),
            None => (None, None, true, None), // unknown command — fall back to files
        }
    };

    let empty: &[String] = &[];
    let flags_slice: &[String] = flags.as_deref().unwrap_or(empty);
    let subs_slice: &[String] = subs.as_deref().unwrap_or(empty);
    let cmd_slice: &[String] = cmd_fallback.as_deref().unwrap_or(empty);

    // Also include files when no static item would survive ranking. Flags are
    // filtered out unless the user is typing one, so a node with only flags and
    // wants_files=false (e.g. harvester mis-tagged grep) would otherwise return
    // nothing. Files are better than an empty list.
    let typing_flag = parsed.current_word.starts_with('-');
    let has_usable_static = flags_slice
        .iter()
        .chain(subs_slice.iter())
        .chain(cmd_slice.iter())
        .any(|s| typing_flag || !s.starts_with('-'));
    let effective_wants_files = wants_files || !has_usable_static;
    let file_items = if effective_wants_files {
        files::list_files(cwd, &parsed.current_word)
    } else {
        vec![]
    };

    let static_buf: Vec<String>;
    let static_slice: &[String] = if !cmd_slice.is_empty() {
        cmd_slice
    } else if subs_slice.is_empty() {
        flags_slice
    } else if flags_slice.is_empty() {
        subs_slice
    } else {
        static_buf = flags_slice
            .iter()
            .chain(subs_slice.iter())
            .cloned()
            .collect();
        &static_buf
    };

    let frecency = state.frecency.read().await;
    let completions =
        ranking::rank_completions(static_slice, &file_items, &parsed.current_word, |s| {
            frecency.score(s)
        });
    drop(frecency);

    // Escape after ranking — the plugin inserts these verbatim via `compadd -Q -U`,
    // so unescaped whitespace/metachars would re-tokenize the command line.
    let completions: Vec<String> = completions
        .into_iter()
        .map(|s| crate::parser::escape_for_shell(&s))
        .collect();

    let mut session = state.sessions.entry(req.session).or_default();
    session.last_seen = std::time::Instant::now();

    if session.is_duplicate(&completions) {
        Response::unchanged()
    } else {
        Response::results(completions)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dashmap::{DashMap, DashSet};
    use std::sync::{Arc, OnceLock};

    fn make_state() -> Arc<crate::SharedState> {
        Arc::new(crate::SharedState {
            tree: Arc::new(crate::cache::CompletionTree::default()),
            cmd_names: OnceLock::new(),
            harvest_in_flight: DashSet::new(),
            harvested: DashMap::new(),
            sessions: DashMap::new(),
            frecency: tokio::sync::RwLock::new(crate::frecency::FrecencyStore::new()),
            frecency_dirty: tokio::sync::Notify::new(),
        })
    }

    #[tokio::test]
    async fn test_root_request_returns_command_names() {
        let state = make_state();
        let _ = state
            .cmd_names
            .set(vec!["cargo".to_string(), "git".to_string()]);
        let tmpdir = tempfile::TempDir::new().unwrap();
        let req = format!(
            "BUFFER=\nCURSOR=0\nCWD={}\nSESSION=1\n",
            tmpdir.path().display()
        );
        let response = process_request(&req, &state).await;
        let completions = response.completions.unwrap_or_default();
        assert!(completions.contains(&"git".to_string()));
        assert!(completions.contains(&"cargo".to_string()));
    }

    #[tokio::test]
    async fn test_valid_request_returns_completions() {
        let state = make_state();
        state.tree.insert(
            &["git".to_string()],
            vec![],
            vec!["add".to_string(), "commit".to_string()],
            false,
        );
        // Mark as already harvested so ensure_harvested doesn't spawn a zsh process
        state.harvested.insert("git".to_string(), ());
        let tmpdir = tempfile::TempDir::new().unwrap();
        let req = format!(
            "BUFFER=git \nCURSOR=4\nCWD={}\nSESSION=2\n",
            tmpdir.path().display()
        );
        let response = process_request(&req, &state).await;
        assert!(!response.unchanged);
        let completions = response.completions.unwrap_or_default();
        assert!(completions.contains(&"add".to_string()));
        assert!(completions.contains(&"commit".to_string()));
    }

    #[tokio::test]
    async fn test_duplicate_request_returns_unchanged() {
        let state = make_state();
        state
            .tree
            .insert(&["git".to_string()], vec![], vec!["add".to_string()], false);
        state.harvested.insert("git".to_string(), ());
        let tmpdir = tempfile::TempDir::new().unwrap();
        let req = format!(
            "BUFFER=git \nCURSOR=4\nCWD={}\nSESSION=3\n",
            tmpdir.path().display()
        );
        let first = process_request(&req, &state).await;
        assert!(!first.unchanged);
        let second = process_request(&req, &state).await;
        assert!(second.unchanged);
    }

    #[tokio::test]
    async fn test_unknown_command_falls_back_to_files() {
        let state = make_state();
        let tmpdir = tempfile::TempDir::new().unwrap();
        std::fs::File::create(tmpdir.path().join("myfile.txt")).unwrap();
        let req = format!(
            "BUFFER=unknowncmd \nCURSOR=11\nCWD={}\nSESSION=4\n",
            tmpdir.path().display()
        );
        let response = process_request(&req, &state).await;
        assert!(!response.unchanged);
        let completions = response.completions.unwrap_or_default();
        assert!(completions.contains(&"myfile.txt".to_string()));
    }

    #[tokio::test]
    async fn test_known_command_only_flags_falls_back_to_files() {
        // grep-style: node has flags but no subcommands, wants_files=false (harvester
        // mis-tag). When the user isn't typing a flag, flags get filtered out by
        // ranking — we must still surface files instead of returning nothing.
        let state = make_state();
        state.tree.insert(
            &["grepish".to_string()],
            vec!["-i".to_string(), "--color".to_string()],
            vec![],
            false,
        );
        state.harvested.insert("grepish".to_string(), ());
        let tmpdir = tempfile::TempDir::new().unwrap();
        std::fs::File::create(tmpdir.path().join("data.txt")).unwrap();
        let req = format!(
            "BUFFER=grepish \nCURSOR=8\nCWD={}\nSESSION=6\n",
            tmpdir.path().display()
        );
        let response = process_request(&req, &state).await;
        let completions = response.completions.unwrap_or_default();
        assert!(completions.contains(&"data.txt".to_string()));
    }

    #[tokio::test]
    #[allow(clippy::float_cmp)]
    async fn test_record_request_updates_frecency() {
        let state = make_state();
        // Score starts at zero.
        assert_eq!(state.frecency.read().await.score("commit"), 0.0);
        // Send a RECORD request.
        let response = process_request("RECORD=commit\n\n", &state).await;
        assert!(response.unchanged);
        // Frecency for "commit" should now be positive.
        assert!(state.frecency.read().await.score("commit") > 0.0);
        // Unrelated item is unchanged.
        assert_eq!(state.frecency.read().await.score("push"), 0.0);
    }

    #[tokio::test]
    async fn test_file_with_space_is_escaped_in_response() {
        let state = make_state();
        let tmpdir = tempfile::TempDir::new().unwrap();
        std::fs::File::create(tmpdir.path().join("my docs.txt")).unwrap();
        let req = format!(
            "BUFFER=cat \nCURSOR=4\nCWD={}\nSESSION=7\n",
            tmpdir.path().display()
        );
        let response = process_request(&req, &state).await;
        let completions = response.completions.unwrap_or_default();
        assert!(
            completions.contains(&"my\\ docs.txt".to_string()),
            "expected escaped filename in {completions:?}"
        );
        // The unescaped form must NOT leak through.
        assert!(!completions.contains(&"my docs.txt".to_string()));
    }

    #[tokio::test]
    #[allow(clippy::float_cmp)]
    async fn test_record_unescapes_before_storing() {
        let state = make_state();
        let response = process_request("RECORD=my\\ docs.txt\n\n", &state).await;
        assert!(response.unchanged);
        assert!(state.frecency.read().await.score("my docs.txt") > 0.0);
        assert_eq!(state.frecency.read().await.score("my\\ docs.txt"), 0.0);
    }

    #[tokio::test]
    async fn test_known_command_no_static_items_falls_back_to_files() {
        // A command node exists with wants_files=false but no flags or subcommands.
        // The daemon must still return file completions rather than nothing.
        let state = make_state();
        state.tree.insert(
            &["mycmd".to_string()],
            vec![], // no flags
            vec![], // no subcommands
            false,  // wants_files explicitly false in tree
        );
        state.harvested.insert("mycmd".to_string(), ());
        let tmpdir = tempfile::TempDir::new().unwrap();
        std::fs::File::create(tmpdir.path().join("report.txt")).unwrap();
        let req = format!(
            "BUFFER=mycmd \nCURSOR=6\nCWD={}\nSESSION=5\n",
            tmpdir.path().display()
        );
        let response = process_request(&req, &state).await;
        assert!(!response.unchanged);
        let completions = response.completions.unwrap_or_default();
        assert!(completions.contains(&"report.txt".to_string()));
    }
}
