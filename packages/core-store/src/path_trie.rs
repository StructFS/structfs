//! A generic prefix trie keyed by path components.
//!
//! `PathTrie<T>` provides O(k) operations where k is the path depth.
//! Each node can optionally hold a value, and has children indexed by path component.

use crate::{path, Path};
use std::collections::BTreeMap;

/// A prefix trie keyed by path components.
///
/// Each node can optionally hold a value of type T, and has children
/// indexed by path component strings. This provides O(k) operations
/// where k is the path depth.
///
/// # Example
///
/// ```rust
/// use structfs_core_store::{PathTrie, path};
///
/// let mut trie: PathTrie<i32> = PathTrie::new();
/// trie.insert(&path!("a/b"), 1);
/// trie.insert(&path!("a/b/c"), 2);
///
/// assert_eq!(trie.get(&path!("a/b")), Some(&1));
///
/// // find_ancestor returns the deepest value along the path
/// let (value, suffix) = trie.find_ancestor(&path!("a/b/c/d")).unwrap();
/// assert_eq!(*value, 2);
/// assert_eq!(suffix, path!("d"));
/// ```
#[derive(Debug, Clone)]
pub struct PathTrie<T> {
    value: Option<T>,
    children: BTreeMap<String, PathTrie<T>>,
}

impl<T> Default for PathTrie<T> {
    fn default() -> Self {
        Self {
            value: None,
            children: BTreeMap::new(),
        }
    }
}

impl<T> PathTrie<T> {
    /// Create an empty trie.
    pub fn new() -> Self {
        Self::default()
    }

    /// Navigate to node, creating intermediate nodes as needed.
    fn get_or_create_node(&mut self, path: &Path) -> &mut PathTrie<T> {
        let mut current = self;
        for component in &path.components {
            current = current.children.entry(component.clone()).or_default();
        }
        current
    }

    /// Navigate to node if it exists.
    fn get_node(&self, path: &Path) -> Option<&PathTrie<T>> {
        let mut current = self;
        for component in &path.components {
            current = current.children.get(component)?;
        }
        Some(current)
    }

    /// Navigate to node if it exists (mutable).
    fn get_node_mut(&mut self, path: &Path) -> Option<&mut PathTrie<T>> {
        let mut current = self;
        for component in &path.components {
            current = current.children.get_mut(component)?;
        }
        Some(current)
    }

    /// Insert a value at path. Returns previous value if any.
    pub fn insert(&mut self, path: &Path, value: T) -> Option<T> {
        let node = self.get_or_create_node(path);
        node.value.replace(value)
    }

    /// Remove and return value at exact path. Children remain.
    pub fn remove(&mut self, path: &Path) -> Option<T> {
        self.get_node_mut(path)?.value.take()
    }

    /// Remove and return entire subtree at path.
    pub fn remove_subtree(&mut self, path: &Path) -> Option<PathTrie<T>> {
        if path.is_empty() {
            let old = std::mem::take(self);
            if old.value.is_some() || !old.children.is_empty() {
                Some(old)
            } else {
                None
            }
        } else {
            let parent_path = Path {
                components: path.components[..path.len() - 1].to_vec(),
            };
            let child_name = &path.components[path.len() - 1];
            let parent = self.get_node_mut(&parent_path)?;
            parent.children.remove(child_name)
        }
    }

    /// Get reference to value at exact path.
    pub fn get(&self, path: &Path) -> Option<&T> {
        self.get_node(path)?.value.as_ref()
    }

    /// Get mutable reference to value at exact path.
    pub fn get_mut(&mut self, path: &Path) -> Option<&mut T> {
        self.get_node_mut(path)?.value.as_mut()
    }

    /// Get reference to subtrie at path.
    pub fn get_subtrie(&self, path: &Path) -> Option<&PathTrie<T>> {
        self.get_node(path)
    }

    /// Get mutable reference to subtrie at path.
    pub fn get_subtrie_mut(&mut self, path: &Path) -> Option<&mut PathTrie<T>> {
        self.get_node_mut(path)
    }

    /// Check if exact path has a value.
    pub fn contains_value(&self, path: &Path) -> bool {
        self.get(path).is_some()
    }

    /// Count of values in trie (not nodes).
    pub fn len(&self) -> usize {
        let self_count = if self.value.is_some() { 1 } else { 0 };
        let children_count: usize = self.children.values().map(|child| child.len()).sum();
        self_count + children_count
    }

    /// True if no values anywhere in trie.
    pub fn is_empty(&self) -> bool {
        self.value.is_none() && self.children.values().all(|c| c.is_empty())
    }

    /// Find deepest ancestor with a value.
    /// Returns (value_ref, remaining_suffix).
    pub fn find_ancestor(&self, path: &Path) -> Option<(&T, Path)> {
        let mut current = self;
        let mut last_value: Option<&T> = self.value.as_ref();
        let mut last_depth: usize = 0;

        for (depth, component) in path.components.iter().enumerate() {
            match current.children.get(component) {
                Some(child) => {
                    current = child;
                    if child.value.is_some() {
                        last_value = child.value.as_ref();
                        last_depth = depth + 1;
                    }
                }
                None => break,
            }
        }

        last_value.map(|v| {
            let suffix = Path {
                components: path.components[last_depth..].to_vec(),
            };
            (v, suffix)
        })
    }

    /// Mutable version of find_ancestor.
    /// Due to borrow checker constraints, this uses a two-pass approach.
    pub fn find_ancestor_mut(&mut self, path: &Path) -> Option<(&mut T, Path)> {
        // First pass: find the depth
        let depth = {
            let mut current = &*self;
            let mut last_depth: usize = if self.value.is_some() { 0 } else { usize::MAX };

            for (d, component) in path.components.iter().enumerate() {
                match current.children.get(component) {
                    Some(child) => {
                        current = child;
                        if child.value.is_some() {
                            last_depth = d + 1;
                        }
                    }
                    None => break,
                }
            }

            if last_depth == usize::MAX {
                return None;
            }
            last_depth
        };

        // Second pass: get mutable reference
        let target_path = Path {
            components: path.components[..depth].to_vec(),
        };
        let suffix = Path {
            components: path.components[depth..].to_vec(),
        };

        self.get_mut(&target_path).map(|v| (v, suffix))
    }

    /// Iterate over all (path, value) pairs.
    pub fn iter(&self) -> PathTrieIter<'_, T> {
        PathTrieIter::new(self)
    }
}

/// Iterator over (Path, &T) pairs in a PathTrie.
pub struct PathTrieIter<'a, T> {
    stack: Vec<(Path, &'a PathTrie<T>)>,
}

impl<'a, T> PathTrieIter<'a, T> {
    fn new(trie: &'a PathTrie<T>) -> Self {
        Self {
            stack: vec![(path!(""), trie)],
        }
    }
}

impl<'a, T> Iterator for PathTrieIter<'a, T> {
    type Item = (Path, &'a T);

    fn next(&mut self) -> Option<Self::Item> {
        while let Some((path, node)) = self.stack.pop() {
            // Push children onto stack (in reverse order for correct iteration)
            for (name, child) in node.children.iter().rev() {
                let child_path = if path.is_empty() {
                    Path {
                        components: vec![name.clone()],
                    }
                } else {
                    Path {
                        components: {
                            let mut c = path.components.clone();
                            c.push(name.clone());
                            c
                        },
                    }
                };
                self.stack.push((child_path, child));
            }

            // Yield this node if it has a value
            if let Some(ref value) = node.value {
                return Some((path, value));
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::path;

    #[test]
    fn new_trie_is_empty() {
        let trie: PathTrie<i32> = PathTrie::new();
        assert!(trie.is_empty());
        assert_eq!(trie.len(), 0);
    }

    #[test]
    fn insert_and_get() {
        let mut trie: PathTrie<i32> = PathTrie::new();
        trie.insert(&path!("a/b"), 42);

        assert_eq!(trie.get(&path!("a/b")), Some(&42));
        assert_eq!(trie.get(&path!("a")), None);
        assert_eq!(trie.get(&path!("a/b/c")), None);
    }

    #[test]
    fn insert_returns_previous() {
        let mut trie: PathTrie<i32> = PathTrie::new();
        assert_eq!(trie.insert(&path!("a"), 1), None);
        assert_eq!(trie.insert(&path!("a"), 2), Some(1));
        assert_eq!(trie.get(&path!("a")), Some(&2));
    }

    #[test]
    fn remove_returns_value() {
        let mut trie: PathTrie<i32> = PathTrie::new();
        trie.insert(&path!("a/b"), 42);

        assert_eq!(trie.remove(&path!("a/b")), Some(42));
        assert_eq!(trie.get(&path!("a/b")), None);
    }

    #[test]
    fn remove_keeps_children() {
        let mut trie: PathTrie<i32> = PathTrie::new();
        trie.insert(&path!("a"), 1);
        trie.insert(&path!("a/b"), 2);

        trie.remove(&path!("a"));

        assert_eq!(trie.get(&path!("a")), None);
        assert_eq!(trie.get(&path!("a/b")), Some(&2));
    }

    #[test]
    fn remove_subtree() {
        let mut trie: PathTrie<i32> = PathTrie::new();
        trie.insert(&path!("a"), 1);
        trie.insert(&path!("a/b"), 2);
        trie.insert(&path!("c"), 3);

        let subtree = trie.remove_subtree(&path!("a")).unwrap();

        assert_eq!(subtree.get(&path!("")), Some(&1));
        assert_eq!(subtree.get(&path!("b")), Some(&2));
        assert_eq!(trie.get(&path!("a")), None);
        assert_eq!(trie.get(&path!("a/b")), None);
        assert_eq!(trie.get(&path!("c")), Some(&3));
    }

    #[test]
    fn remove_subtree_at_root() {
        let mut trie: PathTrie<i32> = PathTrie::new();
        trie.insert(&path!("a"), 1);
        trie.insert(&path!("b"), 2);

        let subtree = trie.remove_subtree(&path!("")).unwrap();

        assert!(trie.is_empty());
        assert_eq!(subtree.get(&path!("a")), Some(&1));
        assert_eq!(subtree.get(&path!("b")), Some(&2));
    }

    #[test]
    fn remove_subtree_nonexistent() {
        let mut trie: PathTrie<i32> = PathTrie::new();
        trie.insert(&path!("a"), 1);

        assert!(trie.remove_subtree(&path!("nonexistent")).is_none());
    }

    #[test]
    fn get_mut() {
        let mut trie: PathTrie<i32> = PathTrie::new();
        trie.insert(&path!("a"), 1);

        *trie.get_mut(&path!("a")).unwrap() = 42;

        assert_eq!(trie.get(&path!("a")), Some(&42));
    }

    #[test]
    fn contains_value() {
        let mut trie: PathTrie<i32> = PathTrie::new();
        trie.insert(&path!("a/b"), 1);

        assert!(trie.contains_value(&path!("a/b")));
        assert!(!trie.contains_value(&path!("a")));
        assert!(!trie.contains_value(&path!("nonexistent")));
    }

    #[test]
    fn len_and_is_empty() {
        let mut trie: PathTrie<i32> = PathTrie::new();
        assert!(trie.is_empty());
        assert_eq!(trie.len(), 0);

        trie.insert(&path!("a"), 1);
        assert!(!trie.is_empty());
        assert_eq!(trie.len(), 1);

        trie.insert(&path!("a/b"), 2);
        assert_eq!(trie.len(), 2);

        trie.insert(&path!("c"), 3);
        assert_eq!(trie.len(), 3);
    }

    #[test]
    fn find_ancestor_basic() {
        let mut trie: PathTrie<&str> = PathTrie::new();
        trie.insert(&path!("data"), "data_store");

        let (value, suffix) = trie.find_ancestor(&path!("data/users/1")).unwrap();
        assert_eq!(*value, "data_store");
        assert_eq!(suffix, path!("users/1"));
    }

    #[test]
    fn find_ancestor_deeper_wins() {
        let mut trie: PathTrie<&str> = PathTrie::new();
        trie.insert(&path!("data"), "data_store");
        trie.insert(&path!("data/cache"), "cache_store");

        // Path that matches cache
        let (value, suffix) = trie.find_ancestor(&path!("data/cache/hot")).unwrap();
        assert_eq!(*value, "cache_store");
        assert_eq!(suffix, path!("hot"));

        // Path that only matches data
        let (value, suffix) = trie.find_ancestor(&path!("data/users/1")).unwrap();
        assert_eq!(*value, "data_store");
        assert_eq!(suffix, path!("users/1"));
    }

    #[test]
    fn find_ancestor_at_root() {
        let mut trie: PathTrie<&str> = PathTrie::new();
        trie.insert(&path!(""), "root_store");

        let (value, suffix) = trie.find_ancestor(&path!("any/path")).unwrap();
        assert_eq!(*value, "root_store");
        assert_eq!(suffix, path!("any/path"));
    }

    #[test]
    fn find_ancestor_no_match() {
        let mut trie: PathTrie<&str> = PathTrie::new();
        trie.insert(&path!("data"), "data_store");

        assert!(trie.find_ancestor(&path!("other/path")).is_none());
    }

    #[test]
    fn find_ancestor_exact_match() {
        let mut trie: PathTrie<&str> = PathTrie::new();
        trie.insert(&path!("data"), "data_store");

        let (value, suffix) = trie.find_ancestor(&path!("data")).unwrap();
        assert_eq!(*value, "data_store");
        assert!(suffix.is_empty());
    }

    #[test]
    fn find_ancestor_mut() {
        let mut trie: PathTrie<i32> = PathTrie::new();
        trie.insert(&path!("a"), 1);
        trie.insert(&path!("a/b"), 2);

        let (value, suffix) = trie.find_ancestor_mut(&path!("a/b/c")).unwrap();
        assert_eq!(*value, 2);
        assert_eq!(suffix, path!("c"));

        // Mutate
        *value = 42;
        assert_eq!(trie.get(&path!("a/b")), Some(&42));
    }

    #[test]
    fn find_ancestor_mut_no_match() {
        let mut trie: PathTrie<i32> = PathTrie::new();
        trie.insert(&path!("a"), 1);

        assert!(trie.find_ancestor_mut(&path!("b/c")).is_none());
    }

    #[test]
    fn iter_all_values() {
        let mut trie: PathTrie<i32> = PathTrie::new();
        trie.insert(&path!("a"), 1);
        trie.insert(&path!("b"), 2);
        trie.insert(&path!("a/c"), 3);

        let mut items: Vec<_> = trie.iter().collect();
        items.sort_by(|a, b| a.0.to_string().cmp(&b.0.to_string()));

        assert_eq!(items.len(), 3);
        assert_eq!(items[0], (path!("a"), &1));
        assert_eq!(items[1], (path!("a/c"), &3));
        assert_eq!(items[2], (path!("b"), &2));
    }

    #[test]
    fn iter_empty() {
        let trie: PathTrie<i32> = PathTrie::new();
        assert_eq!(trie.iter().count(), 0);
    }

    #[test]
    fn iter_root_value() {
        let mut trie: PathTrie<i32> = PathTrie::new();
        trie.insert(&path!(""), 42);

        let items: Vec<_> = trie.iter().collect();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0], (path!(""), &42));
    }

    #[test]
    fn get_subtrie() {
        let mut trie: PathTrie<i32> = PathTrie::new();
        trie.insert(&path!("a/b"), 1);
        trie.insert(&path!("a/c"), 2);

        let subtrie = trie.get_subtrie(&path!("a")).unwrap();
        assert_eq!(subtrie.get(&path!("b")), Some(&1));
        assert_eq!(subtrie.get(&path!("c")), Some(&2));
    }

    #[test]
    fn get_subtrie_nonexistent() {
        let trie: PathTrie<i32> = PathTrie::new();
        assert!(trie.get_subtrie(&path!("nonexistent")).is_none());
    }

    #[test]
    fn clone_trie() {
        let mut trie: PathTrie<i32> = PathTrie::new();
        trie.insert(&path!("a"), 1);

        let cloned = trie.clone();
        assert_eq!(cloned.get(&path!("a")), Some(&1));
    }

    #[test]
    fn default_trie() {
        let trie: PathTrie<i32> = PathTrie::default();
        assert!(trie.is_empty());
    }

    #[test]
    fn debug_trie() {
        let mut trie: PathTrie<i32> = PathTrie::new();
        trie.insert(&path!("a"), 1);
        let debug = format!("{:?}", trie);
        assert!(debug.contains("PathTrie"));
    }

    #[test]
    fn insert_at_root() {
        let mut trie: PathTrie<i32> = PathTrie::new();
        trie.insert(&path!(""), 42);

        assert_eq!(trie.get(&path!("")), Some(&42));
        assert_eq!(trie.len(), 1);
    }

    #[test]
    fn remove_at_root() {
        let mut trie: PathTrie<i32> = PathTrie::new();
        trie.insert(&path!(""), 42);

        assert_eq!(trie.remove(&path!("")), Some(42));
        assert!(trie.is_empty());
    }

    #[test]
    fn get_subtrie_mut() {
        let mut trie: PathTrie<i32> = PathTrie::new();
        trie.insert(&path!("a/b"), 1);

        let subtrie = trie.get_subtrie_mut(&path!("a")).unwrap();
        subtrie.insert(&path!("c"), 2);

        assert_eq!(trie.get(&path!("a/c")), Some(&2));
    }

    #[test]
    fn remove_subtree_empty_trie() {
        let mut trie: PathTrie<i32> = PathTrie::new();
        assert!(trie.remove_subtree(&path!("")).is_none());
    }

    #[test]
    fn is_empty_with_only_structure() {
        let mut trie: PathTrie<i32> = PathTrie::new();
        trie.insert(&path!("a/b/c"), 1);
        trie.remove(&path!("a/b/c"));

        // Trie has structure but no values
        assert!(trie.is_empty());
    }
}
