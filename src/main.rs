mod cache;
mod client;
mod files;
mod frecency;
mod harvester;
mod parser;
mod protocol;
mod ranking;
mod session;
mod socket;

use std::sync::Arc;
use std::sync::OnceLock;

use dashmap::DashMap;
use tokio::sync::{Mutex, Notify};

pub struct SharedState {
    pub tree: Arc<cache::CompletionTree>,
    /// All known command names, populated quickly at startup (no completion functions run).
    pub cmd_names: OnceLock<Vec<String>>,
    /// Per-command watch channel: send(true) when harvest for that command is complete.
    pub harvest_channels: DashMap<String, Arc<tokio::sync::watch::Sender<bool>>>,
    /// Commands whose harvest has fully completed and whose nodes are in `tree`.
    pub harvested: DashMap<String, ()>,
    pub sessions: DashMap<u64, session::SessionState>,
    pub frecency: Mutex<frecency::FrecencyStore>,
    /// Kicked when the frecency store has been mutated; a background task
    /// debounces these kicks and persists the store to disk.
    pub frecency_dirty: Notify,
}

fn socket_path() -> std::path::PathBuf {
    if let Ok(p) = std::env::var("FAST_AUTOCOMPLETE_SOCKET") {
        return std::path::PathBuf::from(p);
    }
    let tmpdir = std::env::var("TMPDIR").map_or_else(
        |_| std::path::PathBuf::from("/tmp"),
        std::path::PathBuf::from,
    );
    let uid = unsafe { libc::getuid() };
    tmpdir.join(format!("fast_autocomplete_{uid}.sock"))
}

fn lock_path() -> std::path::PathBuf {
    let mut p = socket_path();
    p.set_extension("lock");
    p
}

fn frecency_path() -> std::path::PathBuf {
    if let Ok(p) = std::env::var("FAST_AUTOCOMPLETE_FRECENCY_PATH") {
        return std::path::PathBuf::from(p);
    }
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".local/share")))
        .unwrap_or_else(|| std::path::PathBuf::from("/tmp"));
    base.join("fast_autocomplete").join("frecency.bin")
}

/// Acquires an exclusive non-blocking flock on the lock file.
/// Returns the open File (lock is held until the File is dropped).
/// Returns None if another process already holds the lock.
fn try_acquire_lock() -> anyhow::Result<Option<std::fs::File>> {
    use std::os::unix::io::AsRawFd;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(lock_path())?;
    let ret = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if ret == 0 {
        Ok(Some(file))
    } else {
        let err = std::io::Error::last_os_error();
        if err.raw_os_error() == Some(libc::EWOULDBLOCK) {
            Ok(None)
        } else {
            Err(err.into())
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let path = socket_path();
    match args.get(1).map(String::as_str) {
        Some("--complete") => client::run_complete(&path),
        Some("--display") => {
            let cols = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(80);
            client::run_display(&path, cols);
        }
        Some("--record") => {
            let value = args.get(2).map(String::as_str).unwrap_or("");
            client::run_record(&path, value);
        }
        _ => {
            if let Err(e) = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("tokio runtime")
                .block_on(daemon_main())
            {
                eprintln!("fast_autocomplete daemon error: {e}");
                std::process::exit(1);
            }
        }
    }
}

async fn daemon_main() -> anyhow::Result<()> {
    env_logger::init();

    // Acquire exclusive lock before touching the socket — prevents the TOCTOU
    // race where concurrent startups each see no socket and all try to bind.
    let Some(_lock) = try_acquire_lock()? else {
        log::info!("daemon already running (lock held by another process)");
        return Ok(());
    };

    let path = socket_path();

    // Remove a leftover socket from a previously crashed daemon.
    if path.exists() {
        let _ = std::fs::remove_file(&path);
    }

    let listener = tokio::net::UnixListener::bind(&path)?;
    log::info!("listening on {}", path.display());

    let frecency_path = frecency_path();
    let state = Arc::new(SharedState {
        tree: Arc::new(cache::CompletionTree::default()),
        cmd_names: OnceLock::new(),
        harvest_channels: DashMap::new(),
        harvested: DashMap::new(),
        sessions: DashMap::new(),
        frecency: Mutex::new(frecency::FrecencyStore::load(&frecency_path)),
        frecency_dirty: Notify::new(),
    });

    // Debounce task: after each kick, sleep 5s (coalescing further kicks during
    // that window) then persist the frecency store.
    {
        let state = Arc::clone(&state);
        let path = frecency_path.clone();
        tokio::spawn(async move {
            loop {
                state.frecency_dirty.notified().await;
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                let snapshot = state.frecency.lock().await;
                if let Err(e) = snapshot.save(&path) {
                    log::warn!("frecency: save to {} failed: {e}", path.display());
                }
            }
        });
    }

    // Populate command name list quickly at startup (no completion functions run).
    {
        let state = Arc::clone(&state);
        tokio::spawn(async move {
            match tokio::task::spawn_blocking(harvester::list_commands).await {
                Ok(Ok(names)) => {
                    let _ = state.cmd_names.set(names);
                }
                Ok(Err(e)) => log::error!("list_commands failed: {e}"),
                Err(e) => log::error!("list_commands panicked: {e}"),
            }
        });
    }

    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;

    // Accept loop — exits cleanly on SIGINT or SIGTERM so that spawn_blocking
    // threads (and their KillOnDrop child guards) run to completion instead of
    // being abandoned by std::process::exit.
    loop {
        tokio::select! {
            result = listener.accept() => {
                match result {
                    Ok((stream, _)) => {
                        let state = Arc::clone(&state);
                        tokio::spawn(socket::handle_connection(stream, state));
                    }
                    Err(e) => log::error!("accept error: {e}"),
                }
            }
            _ = tokio::signal::ctrl_c() => break,
            _ = sigterm.recv() => break,
        }
    }

    let _ = std::fs::remove_file(&path);

    // Final flush: persist any pending frecency updates before exit.
    {
        let frecency = state.frecency.lock().await;
        if let Err(e) = frecency.save(&frecency_path) {
            log::warn!("frecency: final save to {} failed: {e}", frecency_path.display());
        }
    }

    log::info!("shutdown complete");
    Ok(())
}
