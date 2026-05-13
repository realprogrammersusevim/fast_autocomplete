use dashmap::DashMap;
use std::collections::HashMap;

#[derive(Debug, Default)]
pub struct CompletionNode {
    pub flags: Vec<String>,
    pub subcommands: Vec<String>,
    pub wants_files: bool,
    pub children: HashMap<String, CompletionNode>,
}

#[derive(Debug, Default)]
pub struct CompletionTree {
    pub roots: DashMap<String, CompletionNode>,
}

impl CompletionTree {
    /// Walk words to find the deepest matching node; returns cloned (flags, subcommands, wants_files).
    pub fn lookup(&self, words: &[&str]) -> Option<(Vec<String>, Vec<String>, bool)> {
        if words.is_empty() {
            return None;
        }
        let root = self.roots.get(words[0])?;
        let mut node: &CompletionNode = &*root;
        for word in &words[1..] {
            match node.children.get(*word) {
                Some(child) => node = child,
                None => {
                    // If the word is a known subcommand that hasn't been harvested into
                    // a child node yet, don't leak the parent's sibling subcommands.
                    if node.subcommands.iter().any(|s| s == word) {
                        return Some((vec![], vec![], true));
                    }
                    break;
                }
            }
        }
        Some((
            node.flags.clone(),
            node.subcommands.clone(),
            node.wants_files,
        ))
    }

    /// Insert a node at the given path, creating intermediate nodes as needed.
    pub fn insert(
        &self,
        path: &[String],
        flags: Vec<String>,
        subcommands: Vec<String>,
        wants_files: bool,
    ) {
        if path.is_empty() {
            return;
        }
        if path.len() == 1 {
            let mut entry = self.roots.entry(path[0].clone()).or_default();
            entry.flags = flags;
            entry.subcommands = subcommands;
            entry.wants_files = wants_files;
            return;
        }
        let mut root = self.roots.entry(path[0].clone()).or_default();
        let node = &mut *root;
        let mut current = node;
        for segment in &path[1..path.len() - 1] {
            current = current.children.entry(segment.clone()).or_default();
        }
        let last = path.last().unwrap();
        let child = current.children.entry(last.clone()).or_default();
        child.flags = flags;
        child.subcommands = subcommands;
        child.wants_files = wants_files;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn test_insert_and_lookup_depth_1() {
        let tree = CompletionTree::default();
        tree.insert(
            &["git".to_string()],
            vec!["--version".to_string()],
            vec!["add".to_string()],
            true,
        );
        let result = tree.lookup(&["git"]);
        assert_eq!(result, Some((vec!["--version".to_string()], vec!["add".to_string()], true)));
    }

    #[test]
    fn test_insert_and_lookup_depth_2() {
        let tree = CompletionTree::default();
        tree.insert(
            &["git".to_string(), "commit".to_string()],
            vec!["--amend".to_string()],
            vec![],
            true,
        );
        let result = tree.lookup(&["git", "commit"]);
        assert_eq!(result, Some((vec!["--amend".to_string()], vec![], true)));
        // Intermediate node created with defaults
        let parent = tree.lookup(&["git"]);
        assert_eq!(parent, Some((vec![], vec![], false)));
    }

    #[test]
    fn test_insert_and_lookup_depth_3() {
        let tree = CompletionTree::default();
        tree.insert(
            &["cargo".to_string(), "test".to_string(), "filter".to_string()],
            vec!["--nocapture".to_string()],
            vec![],
            false,
        );
        let result = tree.lookup(&["cargo", "test", "filter"]);
        assert_eq!(result, Some((vec!["--nocapture".to_string()], vec![], false)));
    }

    #[test]
    fn test_lookup_nonexistent_root() {
        let tree = CompletionTree::default();
        assert_eq!(tree.lookup(&["nonexistent"]), None);
    }

    #[test]
    fn test_lookup_unknown_child_falls_back_to_parent() {
        let tree = CompletionTree::default();
        tree.insert(
            &["git".to_string()],
            vec!["--version".to_string()],
            vec!["add".to_string()],
            true,
        );
        // "rebase" is NOT in subcommands, so the break path returns parent node data
        let result = tree.lookup(&["git", "rebase"]);
        assert_eq!(result, Some((vec!["--version".to_string()], vec!["add".to_string()], true)));
    }

    #[test]
    fn test_lookup_known_subcommand_no_child_node() {
        let tree = CompletionTree::default();
        tree.insert(
            &["git".to_string()],
            vec![],
            vec!["add".to_string()],
            true,
        );
        // "add" IS in subcommands but has no child node → special placeholder response
        let result = tree.lookup(&["git", "add"]);
        assert_eq!(result, Some((vec![], vec![], true)));
    }

    #[test]
    fn test_lookup_empty_words() {
        let tree = CompletionTree::default();
        assert_eq!(tree.lookup(&[]), None);
    }

    #[test]
    fn test_insert_overwrites_existing() {
        let tree = CompletionTree::default();
        tree.insert(&["git".to_string()], vec!["--version".to_string()], vec![], false);
        tree.insert(&["git".to_string()], vec!["--help".to_string()], vec![], false);
        let (flags, _, _) = tree.lookup(&["git"]).unwrap();
        assert_eq!(flags, vec!["--help".to_string()]);
    }

    #[test]
    fn test_concurrent_insert_and_lookup() {
        let tree = Arc::new(CompletionTree::default());
        let mut handles = vec![];
        for i in 0..4usize {
            let t = Arc::clone(&tree);
            let h = std::thread::spawn(move || {
                if i < 2 {
                    t.insert(&[format!("cmd{}", i)], vec![], vec![], false);
                } else {
                    let _ = t.lookup(&[&format!("cmd{}", i - 2) as &str]);
                }
            });
            handles.push(h);
        }
        for h in handles {
            h.join().unwrap();
        }
        assert!(tree.lookup(&["cmd0"]).is_some());
        assert!(tree.lookup(&["cmd1"]).is_some());
    }
}
