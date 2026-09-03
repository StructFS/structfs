//! Component-level path patterns for matching, masking, and subscriptions.

use crate::Path;

/// A pattern over paths, matched **component-wise** — never byte-wise.
///
/// A `Prefix` pattern for `config/gate/accounts` matches
/// `config/gate/accounts/personal` but *not* `config/gate/accounts_other`,
/// which a naive string `starts_with` would incorrectly match.
///
/// # Examples
///
/// ```rust
/// use structfs_core_store::{path, PathPattern};
///
/// let exact = PathPattern::exact(path!("gate/defaults/model"));
/// assert!(exact.matches(&path!("gate/defaults/model")));
/// assert!(!exact.matches(&path!("gate/defaults/model/extra")));
///
/// let prefix = PathPattern::prefix(path!("gate/accounts"));
/// assert!(prefix.matches(&path!("gate/accounts")));
/// assert!(prefix.matches(&path!("gate/accounts/personal/key")));
/// assert!(!prefix.matches(&path!("gate/accounts_other")));
///
/// // Match `gate/accounts/{anything...}/provider`
/// let ps = PathPattern::prefix_suffix(path!("gate/accounts"), path!("provider"));
/// assert!(ps.matches(&path!("gate/accounts/personal/provider")));
/// assert!(!ps.matches(&path!("gate/accounts/personal/model")));
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum PathPattern {
    /// Matches exactly one path.
    Exact(Path),
    /// Matches the path itself and everything under it.
    Prefix(Path),
    /// Matches paths that start with the prefix and end with the suffix,
    /// with at least the suffix's components after the prefix. The middle
    /// may be empty: `prefix_suffix(a, c)` matches `a/c` and `a/b/c`.
    PrefixSuffix(Path, Path),
}

impl PathPattern {
    /// Pattern matching exactly `path`.
    pub fn exact(path: Path) -> Self {
        PathPattern::Exact(path)
    }

    /// Pattern matching `path` and all its descendants.
    pub fn prefix(path: Path) -> Self {
        PathPattern::Prefix(path)
    }

    /// Pattern matching paths under `prefix` that end with `suffix`.
    pub fn prefix_suffix(prefix: Path, suffix: Path) -> Self {
        PathPattern::PrefixSuffix(prefix, suffix)
    }

    /// Check whether a path matches this pattern (component-wise).
    pub fn matches(&self, path: &Path) -> bool {
        match self {
            PathPattern::Exact(p) => p == path,
            PathPattern::Prefix(prefix) => path.has_prefix(prefix),
            PathPattern::PrefixSuffix(prefix, suffix) => match path.strip_prefix(prefix) {
                Some(rest) => {
                    rest.len() >= suffix.len()
                        && rest.slice(rest.len() - suffix.len(), rest.len()) == *suffix
                }
                None => false,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::path;

    #[test]
    fn exact_matches_only_itself() {
        let p = PathPattern::exact(path!("a/b"));
        assert!(p.matches(&path!("a/b")));
        assert!(!p.matches(&path!("a")));
        assert!(!p.matches(&path!("a/b/c")));
    }

    #[test]
    fn prefix_is_component_wise() {
        let p = PathPattern::prefix(path!("gate/api_key"));
        assert!(p.matches(&path!("gate/api_key")));
        assert!(p.matches(&path!("gate/api_key/inner")));
        // The byte-prefix bug: "gate/api_key_other" starts with "gate/api_key"
        // as a string, but must not match component-wise.
        assert!(!p.matches(&path!("gate/api_key_other")));
    }

    #[test]
    fn empty_prefix_matches_everything() {
        let p = PathPattern::prefix(path!(""));
        assert!(p.matches(&path!("")));
        assert!(p.matches(&path!("anything/at/all")));
    }

    #[test]
    fn prefix_suffix_middle_may_be_empty() {
        let p = PathPattern::prefix_suffix(path!("accounts"), path!("provider"));
        assert!(p.matches(&path!("accounts/provider")));
        assert!(p.matches(&path!("accounts/personal/provider")));
        assert!(p.matches(&path!("accounts/a/b/provider")));
        assert!(!p.matches(&path!("accounts")));
        assert!(!p.matches(&path!("accounts/personal/model")));
        assert!(!p.matches(&path!("other/personal/provider")));
    }

    #[test]
    fn prefix_suffix_component_wise() {
        let p = PathPattern::prefix_suffix(path!("a"), path!("key"));
        assert!(!p.matches(&path!("a/x/key_other")));
        assert!(p.matches(&path!("a/x/key")));
    }
}
