mod cache;
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
use tokio::sync::Mutex;

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
}

fn socket_path() -> std::path::PathBuf {
    if let Ok(p) = std::env::var("FAST_AUTOCOMPLETE_SOCKET") {
        return std::path::PathBuf::from(p);
    }
    let tmpdir = std::env::var("TMPDIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from("/tmp"));
    let uid = unsafe { libc::getuid() };
    tmpdir.join(format!("fast_autocomplete_{}.sock", uid))
}

fn lock_path() -> std::path::PathBuf {
    let mut p = socket_path();
    p.set_extension("lock");
    p
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

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    env_logger::init();

    // Acquire exclusive lock before touching the socket — prevents the TOCTOU
    // race where concurrent startups each see no socket and all try to bind.
    let _lock = match try_acquire_lock()? {
        Some(f) => f,
        None => {
            log::info!("daemon already running (lock held by another process)");
            return Ok(());
        }
    };

    let path = socket_path();

    // Remove a leftover socket from a previously crashed daemon.
    if path.exists() {
        let _ = std::fs::remove_file(&path);
    }

    let listener = tokio::net::UnixListener::bind(&path)?;
    log::info!("listening on {:?}", path);

    let state = Arc::new(SharedState {
        tree: Arc::new(cache::CompletionTree::default()),
        cmd_names: OnceLock::new(),
        harvest_channels: DashMap::new(),
        harvested: DashMap::new(),
        sessions: DashMap::new(),
        frecency: Mutex::new(frecency::FrecencyStore::new()),
    });

    // Populate command name list quickly at startup (no completion functions run).
    {
        let state = Arc::clone(&state);
        tokio::spawn(async move {
            match tokio::task::spawn_blocking(harvester::list_commands).await {
                Ok(Ok(names)) => {
                    let _ = state.cmd_names.set(names);
                }
                Ok(Err(e)) => log::error!("list_commands failed: {}", e),
                Err(e) => log::error!("list_commands panicked: {}", e),
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
                    Err(e) => log::error!("accept error: {}", e),
                }
            }
            _ = tokio::signal::ctrl_c() => break,
            _ = sigterm.recv() => break,
        }
    }

    let _ = std::fs::remove_file(&path);
    log::info!("shutdown complete");
    Ok(())
}
