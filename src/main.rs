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

use dashmap::{DashMap, DashSet};
use tokio::sync::{Notify, RwLock};

pub struct SharedState {
    pub tree: Arc<cache::CompletionTree>,
    pub cmd_names: OnceLock<Vec<String>>,
    pub harvest_in_flight: DashSet<String>,
    pub harvested: DashMap<String, ()>,
    pub sessions: DashMap<u64, session::SessionState>,
    /// `RwLock` so concurrent score reads don't serialize. Writes are rare.
    pub frecency: RwLock<frecency::FrecencyStore>,
    /// Kicked on frecency mutation; a background task debounces and persists.
    pub frecency_dirty: Notify,
}

fn socket_path() -> std::path::PathBuf {
    if let Ok(p) = std::env::var("FAST_AUTOCOMPLETE_SOCKET") {
        return std::path::PathBuf::from(p);
    }
    // Stable, user-scoped dir so lock/socket paths don't vary with $TMPDIR across launch contexts.
    let uid = unsafe { libc::getuid() };
    let dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .map(|h| std::path::PathBuf::from(h).join(".cache/fast_autocomplete"))
        })
        .unwrap_or_else(|| std::path::PathBuf::from("/tmp"));
    dir.join(format!("fast_autocomplete_{uid}.sock"))
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
        .or_else(|| {
            std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".local/share"))
        })
        .unwrap_or_else(|| std::path::PathBuf::from("/tmp"));
    base.join("fast_autocomplete").join("frecency.bin")
}

/// Acquires an exclusive non-blocking flock on the lock file.
/// Returns the open File (lock is held until the File is dropped).
/// Returns None if another process already holds the lock.
fn try_acquire_lock() -> anyhow::Result<Option<std::fs::File>> {
    use std::os::unix::io::AsRawFd;
    let path = lock_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(&path)?;
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
            let value = args.get(2).map_or("", String::as_str);
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
        harvest_in_flight: DashSet::new(),
        harvested: DashMap::new(),
        sessions: DashMap::new(),
        frecency: RwLock::new(frecency::FrecencyStore::load(&frecency_path)),
        frecency_dirty: Notify::new(),
    });

    {
        let state = Arc::clone(&state);
        let path = frecency_path.clone();
        tokio::spawn(async move {
            loop {
                state.frecency_dirty.notified().await;
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                let mut snapshot = state.frecency.write().await;
                if let Err(e) = snapshot.save(&path) {
                    log::warn!("frecency: save to {} failed: {e}", path.display());
                }
            }
        });
    }

    // Shells come and go; without eviction the sessions map grows monotonically.
    {
        let state = Arc::clone(&state);
        tokio::spawn(async move {
            const SWEEP_INTERVAL: std::time::Duration = std::time::Duration::from_secs(300);
            const SESSION_TTL: std::time::Duration = std::time::Duration::from_secs(3600);
            loop {
                tokio::time::sleep(SWEEP_INTERVAL).await;
                let now = std::time::Instant::now();
                state
                    .sessions
                    .retain(|_, s| now.duration_since(s.last_seen) < SESSION_TTL);
            }
        });
    }

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

    // Clean exit on signal so KillOnDrop guards run to completion rather than being abandoned.
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

    {
        let mut frecency = state.frecency.write().await;
        if let Err(e) = frecency.save(&frecency_path) {
            log::warn!(
                "frecency: final save to {} failed: {e}",
                frecency_path.display()
            );
        }
    }

    log::info!("shutdown complete");
    Ok(())
}
