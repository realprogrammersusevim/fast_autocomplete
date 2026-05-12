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
                None => break,
            }
        }
        Some((node.flags.clone(), node.subcommands.clone(), node.wants_files))
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
