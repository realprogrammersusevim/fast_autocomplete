use std::path::{Path, PathBuf};

/// List immediate directory entries that match the given partial path prefix.
///
/// Handles relative paths, absolute paths, and simple ~ expansion.
/// Directories are returned with a trailing `/`.
/// Hidden files are included only if `partial` starts with `.`.
pub fn list_files(cwd: &Path, partial: &str) -> Vec<String> {
    let (dir_part, name_prefix) = match partial.rfind('/') {
        Some(i) => (&partial[..=i], &partial[i + 1..]),
        None => ("", partial),
    };

    let target_dir = resolve_dir(cwd, dir_part);

    let mut results = Vec::new();
    let Ok(entries) = std::fs::read_dir(&target_dir) else {
        return results;
    };

    let show_hidden = name_prefix.starts_with('.');

    for entry in entries.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();

        if !show_hidden && name_str.starts_with('.') {
            continue;
        }

        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        let display = format!("{}{}{}", dir_part, name_str, if is_dir { "/" } else { "" });
        results.push(display);
    }

    results.sort();
    results
}

fn resolve_dir(cwd: &Path, dir_part: &str) -> PathBuf {
    if dir_part.is_empty() {
        return cwd.to_path_buf();
    }
    if dir_part.starts_with('/') {
        return PathBuf::from(dir_part);
    }
    if let Some(rest) = dir_part.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    if dir_part == "~" || dir_part == "~/" {
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home);
        }
    }
    cwd.join(dir_part)
}
