use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

use crate::protocol::{Request, Response};
use crate::{SharedState, files, harvester, ranking};

/// Handle one incoming connection: read a single request block, respond, close.
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
        let _ = writer.write_all(format!("{json}\n").as_bytes()).await;
    }
}

/// Trigger a JIT harvest for `cmd` if one hasn't started yet. Returns immediately;
/// the harvest runs in the background and populates the tree for subsequent requests.
fn ensure_harvested(cmd: &str, state: &Arc<SharedState>) {
    if state.harvested.contains_key(cmd) || state.harvest_channels.contains_key(cmd) {
        return;
    }

    let (tx, _) = tokio::sync::watch::channel(false);
    let tx = Arc::new(tx);
    state
        .harvest_channels
        .insert(cmd.to_string(), Arc::clone(&tx));

    let cmd_owned = cmd.to_string();
    let state_clone = Arc::clone(state);
    tokio::task::spawn_blocking(move || {
        if let Err(e) = harvester::harvest_command(&cmd_owned, &state_clone.tree) {
            log::warn!("harvest_command('{cmd_owned}') failed: {e}");
        }
        state_clone.harvested.insert(cmd_owned, ());
        let _ = tx.send(true);
    });
}

async fn process_request(raw: &str, state: &Arc<SharedState>) -> Response {
    let req = Request::parse(raw);

    let parsed = crate::parser::parse_buffer(&req.buffer, req.cursor);
    let cwd = std::path::Path::new(&req.cwd);

    // JIT: trigger harvest for the top-level command if not yet done.
    if let Some(cmd) = parsed.lookup_words.first() {
        ensure_harvested(cmd, state);
    }

    let (static_items, wants_files) = if parsed.lookup_words.is_empty() {
        // Completing the command name itself — return all known command names.
        let cmds = match state.cmd_names.get() {
            Some(names) => names.clone(),
            None => state.tree.roots.iter().map(|e| e.key().clone()).collect(),
        };
        (cmds, true)
    } else {
        let word_refs: Vec<&str> = parsed.lookup_words.iter().map(String::as_str).collect();
        match state.tree.lookup(&word_refs) {
            Some((flags, subs, wants_files)) => {
                let mut items = flags;
                items.extend(subs);
                (items, wants_files)
            }
            None => (vec![], true), // unknown command — fall back to files
        }
    };

    // Also include files when static_items is empty — if the tree node exists but
    // produced no usable completions (harvester limitation, stale data, etc.),
    // files are better than nothing.
    let effective_wants_files = wants_files || static_items.is_empty();
    let file_items = if effective_wants_files {
        files::list_files(cwd, &parsed.current_word)
    } else {
        vec![]
    };

    // Snapshot only the scores we need, then release the lock before the CPU-heavy ranking.
    let frecency_scores = {
        let frecency = state.frecency.lock().await;
        static_items
            .iter()
            .chain(file_items.iter())
            .map(|s| (s.clone(), frecency.score(s)))
            .collect::<std::collections::HashMap<String, f64>>()
    };

    let completions = ranking::rank_completions(
        static_items,
        file_items,
        &parsed.current_word,
        &frecency_scores,
    );

    let mut session = state.sessions.entry(req.session).or_default();

    if session.is_duplicate(&completions) {
        Response::unchanged()
    } else {
        Response::results(completions)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dashmap::DashMap;
    use std::sync::{Arc, OnceLock};

    fn make_state() -> Arc<crate::SharedState> {
        Arc::new(crate::SharedState {
            tree: Arc::new(crate::cache::CompletionTree::default()),
            cmd_names: OnceLock::new(),
            harvest_channels: DashMap::new(),
            harvested: DashMap::new(),
            sessions: DashMap::new(),
            frecency: tokio::sync::Mutex::new(crate::frecency::FrecencyStore::new()),
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
    async fn test_known_command_no_static_items_falls_back_to_files() {
        // A command node exists with wants_files=false but no flags or subcommands.
        // The daemon must still return file completions rather than nothing.
        let state = make_state();
        state.tree.insert(
            &["mycmd".to_string()],
            vec![],  // no flags
            vec![],  // no subcommands
            false,   // wants_files explicitly false in tree
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
