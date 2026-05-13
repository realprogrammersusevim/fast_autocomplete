use std::sync::Arc;
use std::time::Duration;
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
        let _ = writer.write_all(format!("{}\n", json).as_bytes()).await;
    }
}

/// Ensure completions for `cmd` are in the tree, triggering a JIT harvest if needed.
/// Waits up to 300 ms for the harvest to finish before returning.
async fn ensure_harvested(cmd: &str, state: &Arc<SharedState>) {
    if state.harvested.contains_key(cmd) {
        return;
    }

    // Get or create the watch channel for this command.
    let mut is_new = false;
    let tx = state
        .harvest_channels
        .entry(cmd.to_string())
        .or_insert_with(|| {
            is_new = true;
            let (tx, _) = tokio::sync::watch::channel(false);
            Arc::new(tx)
        })
        .clone();

    if is_new {
        let cmd_owned = cmd.to_string();
        let state_clone = Arc::clone(state);
        let tx_clone = Arc::clone(&tx);
        tokio::task::spawn_blocking(move || {
            if let Err(e) = harvester::harvest_command(&cmd_owned, &state_clone.tree) {
                log::warn!("harvest_command('{}') failed: {}", cmd_owned, e);
            }
            state_clone.harvested.insert(cmd_owned, ());
            let _ = tx_clone.send(true);
        });
    }

    // subscribe() starts at the current channel value, so if send(true) already
    // happened we see it immediately without any await.
    let mut rx = tx.subscribe();
    if *rx.borrow() {
        return;
    }
    let _ = tokio::time::timeout(Duration::from_millis(300), rx.wait_for(|v| *v)).await;
}

async fn process_request(raw: &str, state: &Arc<SharedState>) -> Response {
    let Some(req) = Request::parse(raw) else {
        return Response::error("parse_error");
    };

    let parsed = crate::parser::parse_buffer(&req.buffer, req.cursor);
    let cwd = std::path::Path::new(&req.cwd);

    // JIT: trigger harvest for the top-level command if not yet done.
    if let Some(cmd) = parsed.lookup_words.first() {
        ensure_harvested(cmd, state).await;
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

    let file_items = if wants_files {
        files::list_files(cwd, &parsed.current_word)
    } else {
        vec![]
    };

    let frecency = state.frecency.lock().await;
    let completions =
        ranking::rank_completions(static_items, file_items, &parsed.current_word, &frecency);
    drop(frecency);

    let mut session = state
        .sessions
        .entry(req.session)
        .or_insert_with(crate::session::SessionState::default);

    if session.is_duplicate(&completions) {
        Response::unchanged()
    } else {
        Response::results(completions)
    }
}
