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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_list_files_simple() {
        let dir = tempfile::TempDir::new().unwrap();
        fs::File::create(dir.path().join("foo.txt")).unwrap();
        fs::File::create(dir.path().join("bar.txt")).unwrap();
        let result = list_files(dir.path(), "");
        assert!(result.contains(&"foo.txt".to_string()));
        assert!(result.contains(&"bar.txt".to_string()));
    }

    #[test]
    fn test_directory_gets_trailing_slash() {
        let dir = tempfile::TempDir::new().unwrap();
        fs::create_dir(dir.path().join("mydir")).unwrap();
        let result = list_files(dir.path(), "");
        assert!(result.contains(&"mydir/".to_string()));
    }

    #[test]
    fn test_hidden_files_excluded_by_default() {
        let dir = tempfile::TempDir::new().unwrap();
        fs::File::create(dir.path().join(".hidden")).unwrap();
        fs::File::create(dir.path().join("visible")).unwrap();
        let result = list_files(dir.path(), "");
        assert!(result.contains(&"visible".to_string()));
        assert!(!result.contains(&".hidden".to_string()));
    }

    #[test]
    fn test_hidden_files_shown_with_dot_prefix() {
        let dir = tempfile::TempDir::new().unwrap();
        fs::File::create(dir.path().join(".hidden")).unwrap();
        let result = list_files(dir.path(), ".");
        assert!(result.contains(&".hidden".to_string()));
    }

    #[test]
    fn test_nonexistent_dir_returns_empty() {
        let result = list_files(Path::new("/nonexistent_xyzzy_fast_autocomplete_test"), "");
        assert!(result.is_empty());
    }

    #[test]
    fn test_relative_subdir() {
        let dir = tempfile::TempDir::new().unwrap();
        let subdir = dir.path().join("sub");
        fs::create_dir(&subdir).unwrap();
        fs::File::create(subdir.join("file.txt")).unwrap();
        let result = list_files(dir.path(), "sub/");
        assert!(result.contains(&"sub/file.txt".to_string()));
    }

    #[test]
    fn test_absolute_partial_path() {
        let dir = tempfile::TempDir::new().unwrap();
        fs::File::create(dir.path().join("alpha.txt")).unwrap();
        let partial = format!("{}/", dir.path().display());
        let result = list_files(Path::new("/irrelevant"), &partial);
        let expected = format!("{}/alpha.txt", dir.path().display());
        assert!(result.contains(&expected), "expected {:?} in {:?}", expected, result);
    }

    #[test]
    fn test_partial_prefix_no_filtering() {
        // list_files does NOT filter by name_prefix; fuzzy filtering is in rank_completions
        let dir = tempfile::TempDir::new().unwrap();
        fs::File::create(dir.path().join("alpha.txt")).unwrap();
        fs::File::create(dir.path().join("beta.txt")).unwrap();
        fs::File::create(dir.path().join("almond.txt")).unwrap();
        let result = list_files(dir.path(), "al");
        assert!(result.contains(&"alpha.txt".to_string()));
        assert!(result.contains(&"beta.txt".to_string()));
        assert!(result.contains(&"almond.txt".to_string()));
    }
}
