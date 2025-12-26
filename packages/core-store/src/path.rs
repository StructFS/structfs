//! Path type with validated Unicode identifier components.

use std::fmt;

/// Errors related to path parsing and validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathError {
    /// A path component is not a valid Unicode identifier.
    InvalidComponent {
        component: String,
        position: usize,
        message: String,
    },
    /// The path string is invalid.
    InvalidPath { message: String },
}

impl fmt::Display for PathError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PathError::InvalidComponent {
                component,
                position,
                message,
            } => {
                write!(
                    f,
                    "invalid path component '{}' at position {}: {}",
                    component, position, message
                )
            }
            PathError::InvalidPath { message } => {
                write!(f, "invalid path: {}", message)
            }
        }
    }
}

impl std::error::Error for PathError {}

/// A validated path in StructFS.
///
/// Path components must be valid Unicode identifiers (per UAX#31) or
/// numeric strings (for array indexing). This ensures paths can be
/// used as identifiers in most programming languages.
#[derive(Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub struct Path {
    pub components: Vec<String>,
}

impl Path {
    /// Parse a path string, validating components.
    ///
    /// # Path Syntax
    ///
    /// - Components are separated by `/`
    /// - Empty components are ignored (normalizes `//` and trailing `/`)
    /// - Each component must be a valid identifier or numeric string
    ///
    /// # Examples
    ///
    /// ```rust
    /// use structfs_core_store::Path;
    ///
    /// let path = Path::parse("users/123/name").unwrap();
    /// assert_eq!(path.len(), 3);
    ///
    /// // Trailing slashes are normalized
    /// assert_eq!(Path::parse("foo/bar/").unwrap(), Path::parse("foo/bar").unwrap());
    /// ```
    pub fn parse(s: &str) -> Result<Self, PathError> {
        if s.is_empty() {
            return Ok(Path {
                components: Vec::new(),
            });
        }

        let components: Vec<String> = s
            .split('/')
            .filter(|c| !c.is_empty())
            .map(|c| c.to_string())
            .collect();

        // Validate each component
        for (i, component) in components.iter().enumerate() {
            Self::validate_component(component, i)?;
        }

        Ok(Path { components })
    }

    /// Create a path from pre-validated components.
    ///
    /// # Panics
    ///
    /// Panics if any component is invalid. Use `try_from_components` for
    /// fallible construction.
    pub fn from_components(components: Vec<String>) -> Self {
        for (i, component) in components.iter().enumerate() {
            Self::validate_component(component, i).expect("invalid component");
        }
        Path { components }
    }

    /// Try to create a path from components, validating each.
    pub fn try_from_components(components: Vec<String>) -> Result<Self, PathError> {
        for (i, component) in components.iter().enumerate() {
            Self::validate_component(component, i)?;
        }
        Ok(Path { components })
    }

    /// Validate a single path component.
    fn validate_component(component: &str, position: usize) -> Result<(), PathError> {
        if component.is_empty() {
            return Err(PathError::InvalidComponent {
                component: component.to_string(),
                position,
                message: "empty component".to_string(),
            });
        }

        // Allow pure numeric strings (for array indexing)
        if component.chars().all(|c| c.is_ascii_digit()) {
            return Ok(());
        }

        // Check for valid identifier
        let mut chars = component.chars();
        let first = chars.next().unwrap();

        // First char: XID_Start or underscore followed by XID_Continue
        let valid_start = unicode_ident::is_xid_start(first)
            || (first == '_'
                && chars
                    .clone()
                    .next()
                    .is_some_and(unicode_ident::is_xid_continue));

        if !valid_start {
            return Err(PathError::InvalidComponent {
                component: component.to_string(),
                position,
                message: "must start with a letter or underscore followed by letter/digit"
                    .to_string(),
            });
        }

        // Rest: XID_Continue
        for c in chars {
            if !unicode_ident::is_xid_continue(c) {
                return Err(PathError::InvalidComponent {
                    component: component.to_string(),
                    position,
                    message: format!("invalid character '{}' in identifier", c),
                });
            }
        }

        Ok(())
    }

    /// Check if this path is empty (root path).
    pub fn is_empty(&self) -> bool {
        self.components.is_empty()
    }

    /// Get the number of components.
    pub fn len(&self) -> usize {
        self.components.len()
    }

    /// Iterate over components.
    pub fn iter(&self) -> impl Iterator<Item = &String> {
        self.components.iter()
    }

    /// Join this path with another.
    #[must_use]
    pub fn join(&self, other: &Path) -> Path {
        let mut components = self.components.clone();
        components.extend(other.components.iter().cloned());
        Path { components }
    }

    /// Check if this path has the given prefix.
    pub fn has_prefix(&self, prefix: &Path) -> bool {
        prefix.components.len() <= self.components.len()
            && prefix.components == self.components[..prefix.components.len()]
    }

    /// Strip a prefix from this path.
    ///
    /// Returns `None` if the prefix doesn't match.
    #[must_use]
    pub fn strip_prefix(&self, prefix: &Path) -> Option<Path> {
        if self.has_prefix(prefix) {
            Some(Path {
                components: self.components[prefix.components.len()..].to_vec(),
            })
        } else {
            None
        }
    }

    /// Get a slice of components as a new path.
    pub fn slice(&self, start: usize, end: usize) -> Path {
        Path {
            components: self.components[start..end].to_vec(),
        }
    }

    /// Convert to LL path (byte components).
    pub fn to_ll_path(&self) -> structfs_ll_store::LLPath {
        self.components
            .iter()
            .map(|c| bytes::Bytes::copy_from_slice(c.as_bytes()))
            .collect()
    }

    /// Try to create from LL path (byte components).
    ///
    /// Fails if any component is not valid UTF-8 or not a valid identifier.
    pub fn try_from_ll_path(ll_path: &[impl AsRef<[u8]>]) -> Result<Self, PathError> {
        let mut components = Vec::with_capacity(ll_path.len());
        for (i, bytes) in ll_path.iter().enumerate() {
            let s =
                std::str::from_utf8(bytes.as_ref()).map_err(|_| PathError::InvalidComponent {
                    component: format!("{:?}", bytes.as_ref()),
                    position: i,
                    message: "not valid UTF-8".to_string(),
                })?;
            Self::validate_component(s, i)?;
            components.push(s.to_string());
        }
        Ok(Path { components })
    }
}

impl fmt::Display for Path {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.components.join("/"))
    }
}

impl std::ops::Index<usize> for Path {
    type Output = String;

    fn index(&self, i: usize) -> &Self::Output {
        &self.components[i]
    }
}

/// Macro for creating paths at compile time.
///
/// # Example
///
/// ```rust
/// use structfs_core_store::path;
///
/// let p = path!("users/123/name");
/// assert_eq!(p.len(), 3);
/// ```
#[macro_export]
macro_rules! path {
    ($s:expr) => {
        $crate::Path::parse($s).expect("invalid path literal")
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_basic_paths() {
        assert_eq!(Path::parse("").unwrap().len(), 0);
        assert_eq!(Path::parse("foo").unwrap().len(), 1);
        assert_eq!(Path::parse("foo/bar").unwrap().len(), 2);
        assert_eq!(Path::parse("foo/bar/baz").unwrap().len(), 3);
    }

    #[test]
    fn normalize_slashes() {
        assert_eq!(
            Path::parse("foo/bar/").unwrap(),
            Path::parse("foo/bar").unwrap()
        );
        assert_eq!(
            Path::parse("foo//bar").unwrap(),
            Path::parse("foo/bar").unwrap()
        );
        assert_eq!(
            Path::parse("/foo/bar").unwrap(),
            Path::parse("foo/bar").unwrap()
        );
    }

    #[test]
    fn numeric_components_allowed() {
        let p = Path::parse("items/0/name").unwrap();
        assert_eq!(p.len(), 3);
        assert_eq!(&p[1], "0");
    }

    #[test]
    fn unicode_identifiers_allowed() {
        let p = Path::parse("usuarios/名前").unwrap();
        assert_eq!(p.len(), 2);
    }

    #[test]
    fn invalid_components_rejected() {
        assert!(Path::parse("foo/bar baz").is_err()); // space
        assert!(Path::parse("foo/bar-baz").is_err()); // hyphen
        assert!(Path::parse("foo/.hidden").is_err()); // starts with dot
        assert!(Path::parse("foo/123abc").is_err()); // starts with digit but not pure numeric
    }

    #[test]
    fn has_prefix_works() {
        let p = path!("foo/bar/baz");
        assert!(p.has_prefix(&path!("")));
        assert!(p.has_prefix(&path!("foo")));
        assert!(p.has_prefix(&path!("foo/bar")));
        assert!(p.has_prefix(&path!("foo/bar/baz")));
        assert!(!p.has_prefix(&path!("bar")));
        assert!(!p.has_prefix(&path!("foo/bar/baz/qux")));
    }

    #[test]
    fn strip_prefix_works() {
        let p = path!("foo/bar/baz");
        assert_eq!(p.strip_prefix(&path!("foo")), Some(path!("bar/baz")));
        assert_eq!(p.strip_prefix(&path!("foo/bar")), Some(path!("baz")));
        assert_eq!(p.strip_prefix(&path!("other")), None);
    }

    #[test]
    fn ll_conversion_roundtrips() {
        let p = path!("users/123/name");
        let ll = p.to_ll_path();
        let p2 = Path::try_from_ll_path(&ll.iter().collect::<Vec<_>>()).unwrap();
        assert_eq!(p, p2);
    }

    #[test]
    fn path_error_display_invalid_component() {
        let err = PathError::InvalidComponent {
            component: "bad-name".to_string(),
            position: 2,
            message: "test message".to_string(),
        };
        let display = format!("{}", err);
        assert!(display.contains("bad-name"));
        assert!(display.contains("position 2"));
        assert!(display.contains("test message"));
    }

    #[test]
    fn path_error_display_invalid_path() {
        let err = PathError::InvalidPath {
            message: "some reason".to_string(),
        };
        let display = format!("{}", err);
        assert!(display.contains("invalid path"));
        assert!(display.contains("some reason"));
    }

    #[test]
    fn path_error_is_error() {
        let err: Box<dyn std::error::Error> = Box::new(PathError::InvalidPath {
            message: "test".to_string(),
        });
        let _ = err.to_string();
    }

    #[test]
    fn from_components_valid() {
        let p = Path::from_components(vec!["foo".to_string(), "bar".to_string()]);
        assert_eq!(p.len(), 2);
    }

    #[test]
    #[should_panic(expected = "invalid component")]
    fn from_components_invalid_panics() {
        Path::from_components(vec!["foo".to_string(), "bad-name".to_string()]);
    }

    #[test]
    fn try_from_components_valid() {
        let p = Path::try_from_components(vec!["foo".to_string(), "bar".to_string()]).unwrap();
        assert_eq!(p.len(), 2);
    }

    #[test]
    fn try_from_components_invalid() {
        let result = Path::try_from_components(vec!["foo".to_string(), "bad-name".to_string()]);
        assert!(result.is_err());
    }

    #[test]
    fn validate_empty_component_rejected() {
        let result = Path::try_from_components(vec!["".to_string()]);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("empty component"));
    }

    #[test]
    fn validate_underscore_alone_rejected() {
        // Underscore alone without follow-up character should be rejected
        let result = Path::parse("_");
        assert!(result.is_err());
    }

    #[test]
    fn validate_underscore_with_continuation_allowed() {
        // _foo is valid
        let p = Path::parse("_foo").unwrap();
        assert_eq!(p.len(), 1);
    }

    #[test]
    fn validate_invalid_character_in_middle() {
        let result = Path::parse("foo$bar");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("invalid character"));
    }

    #[test]
    fn index_trait() {
        let p = path!("foo/bar/baz");
        assert_eq!(&p[0], "foo");
        assert_eq!(&p[1], "bar");
        assert_eq!(&p[2], "baz");
    }

    #[test]
    fn slice_method() {
        let p = path!("a/b/c/d");
        let sliced = p.slice(1, 3);
        assert_eq!(sliced.len(), 2);
        assert_eq!(sliced.to_string(), "b/c");
    }

    #[test]
    fn join_method() {
        let p1 = path!("foo/bar");
        let p2 = path!("baz/qux");
        let joined = p1.join(&p2);
        assert_eq!(joined.to_string(), "foo/bar/baz/qux");
    }

    #[test]
    fn join_with_empty() {
        let p1 = path!("foo");
        let p2 = path!("");
        assert_eq!(p1.join(&p2), p1);

        let p3 = path!("");
        let p4 = path!("bar");
        assert_eq!(p3.join(&p4), p4);
    }

    #[test]
    fn iter_method() {
        let p = path!("a/b/c");
        let components: Vec<&String> = p.iter().collect();
        assert_eq!(components.len(), 3);
        assert_eq!(components[0], "a");
        assert_eq!(components[1], "b");
        assert_eq!(components[2], "c");
    }

    #[test]
    fn is_empty() {
        assert!(path!("").is_empty());
        assert!(!path!("foo").is_empty());
    }

    #[test]
    fn display_impl() {
        let p = path!("foo/bar/baz");
        assert_eq!(format!("{}", p), "foo/bar/baz");
    }

    #[test]
    fn display_empty() {
        let p = path!("");
        assert_eq!(format!("{}", p), "");
    }

    #[test]
    fn ll_conversion_invalid_utf8() {
        let invalid_utf8: Vec<&[u8]> = vec![&[0xff, 0xfe]];
        let result = Path::try_from_ll_path(&invalid_utf8);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("not valid UTF-8"));
    }

    #[test]
    fn path_ord() {
        let p1 = path!("a/b");
        let p2 = path!("a/c");
        let p3 = path!("b/a");
        assert!(p1 < p2);
        assert!(p2 < p3);
    }

    #[test]
    fn path_hash() {
        use std::collections::HashSet;
        let mut set = HashSet::new();
        set.insert(path!("foo"));
        set.insert(path!("bar"));
        set.insert(path!("foo")); // duplicate
        assert_eq!(set.len(), 2);
    }

    #[test]
    fn path_clone() {
        let p1 = path!("foo/bar");
        let p2 = p1.clone();
        assert_eq!(p1, p2);
    }

    #[test]
    fn path_debug() {
        let p = path!("foo/bar");
        let debug = format!("{:?}", p);
        assert!(debug.contains("foo"));
        assert!(debug.contains("bar"));
    }
}
