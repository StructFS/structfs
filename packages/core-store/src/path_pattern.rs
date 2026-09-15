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
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PathPattern {
    /// Matches exactly one path.
    Exact(Path),
    /// Matches the path itself and everything under it.
    Prefix(Path),
    /// Prefix and suffix cannot overlap. `min_middle` counts intervening components.
    PrefixSuffix {
        prefix: Path,
        suffix: Path,
        min_middle: usize,
    },
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
        PathPattern::PrefixSuffix {
            prefix,
            suffix,
            min_middle: 0,
        }
    }

    /// Match with an explicit minimum middle length. Zero retains the
    /// `prefix_suffix` behavior; one requires at least one intervening component.
    /// The shorter constructor selects zero; both serialize to the same variant.
    pub fn prefix_suffix_with_min_middle(prefix: Path, suffix: Path, min_middle: usize) -> Self {
        Self::PrefixSuffix {
            prefix,
            suffix,
            min_middle,
        }
    }

    /// Check whether a path matches this pattern (component-wise).
    pub fn matches(&self, path: &Path) -> bool {
        match self {
            PathPattern::Exact(p) => p == path,
            PathPattern::Prefix(prefix) => path.has_prefix(prefix),
            PathPattern::PrefixSuffix {
                prefix,
                suffix,
                min_middle,
            } => matches_prefix_suffix(path, prefix, suffix, *min_middle),
        }
    }
}

/// Allocation-free component predicate for callers retaining their own pattern representation.
/// Prefix and suffix must be disjoint; empty paths and all minimum lengths are supported.
pub fn matches_prefix_suffix(path: &Path, prefix: &Path, suffix: &Path, min_middle: usize) -> bool {
    let Some(rest) = path.len().checked_sub(prefix.len()) else {
        return false;
    };
    let Some(middle) = rest.checked_sub(suffix.len()) else {
        return false;
    };
    middle >= min_middle
        && path.has_prefix(prefix)
        && path
            .iter()
            .skip(path.len() - suffix.len())
            .eq(suffix.iter())
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

#[cfg(test)]
mod policy_tests {
    use super::*;
    use crate::path;
    #[test]
    fn explicit_policy_and_serde_preserve_watch_boundaries() {
        let pattern =
            PathPattern::prefix_suffix_with_min_middle(path!("accounts"), path!("provider"), 1);
        let json = r#"{"prefix_suffix":{"prefix":"accounts","suffix":"provider","min_middle":1}}"#;
        assert_eq!(serde_json::to_string(&pattern).unwrap(), json);
        assert_eq!(serde_json::from_str::<PathPattern>(json).unwrap(), pattern);
        assert!(!pattern.matches(&path!("accounts/provider")));
        assert!(pattern.matches(&path!("accounts/alice/provider")));
        assert!(pattern.matches(&path!("accounts/a/b/provider")));
        assert!(!pattern.matches(&path!("accounts/a/provider_other")));
        let old = PathPattern::prefix_suffix(path!("accounts"), path!("provider"));
        assert!(old.matches(&path!("accounts/provider")));
        assert_eq!(
            serde_json::to_string(&old).unwrap(),
            r#"{"prefix_suffix":{"prefix":"accounts","suffix":"provider","min_middle":0}}"#
        );
        assert!(
            !PathPattern::prefix_suffix_with_min_middle(path!(""), path!(""), usize::MAX)
                .matches(&path!("a"))
        );
        assert!(
            !PathPattern::prefix_suffix_with_min_middle(path!("a"), path!("a/b"), 0)
                .matches(&path!("a/b"))
        );
        assert!(
            PathPattern::prefix_suffix_with_min_middle(path!(""), path!(""), 1)
                .matches(&path!("a"))
        );
    }
}
