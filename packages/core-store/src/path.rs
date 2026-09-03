//! Path type with validated Unicode identifier components.

use std::fmt;

/// Errors related to path parsing and validation.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
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
    pub(crate) components: Vec<String>,
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

    /// Create a path from components that are already known to be valid.
    ///
    /// This is the construction path used by the `path!` macro: literals are
    /// validated at compile time and expressions are `PathComponent` values
    /// validated at construction, so no runtime re-validation is needed.
    /// Debug builds re-check as a safety net.
    ///
    /// Prefer `from_components`/`try_from_components` for strings whose
    /// validity is not already guaranteed.
    #[doc(hidden)]
    pub fn from_validated_components(components: Vec<String>) -> Self {
        #[cfg(debug_assertions)]
        for (i, component) in components.iter().enumerate() {
            Self::validate_component(component, i).expect("invalid pre-validated component");
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

    /// Validate a single path component against the StructFS grammar.
    ///
    /// The grammar is shared with the compile-time `path!` macro via the
    /// `structfs-path-validation` crate: a component is a UAX#31 identifier
    /// (an underscore prefix is allowed when followed by more identifier
    /// characters) or a pure numeric string.
    ///
    /// `position` is only used to build the error; pass `0` when validating
    /// a component in isolation.
    pub fn validate_component(component: &str, position: usize) -> Result<(), PathError> {
        structfs_path_validation::validate_component(component).map_err(|message| {
            PathError::InvalidComponent {
                component: component.to_string(),
                position,
                message,
            }
        })
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

    /// Return a new path with the component appended.
    #[must_use]
    pub fn child(&self, component: impl Into<PathComponent>) -> Path {
        let mut components = self.components.clone();
        components.push(component.into().into_string());
        Path { components }
    }

    /// Append a component in place.
    pub fn push(&mut self, component: impl Into<PathComponent>) {
        self.components.push(component.into().into_string());
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

/// A single validated path component.
///
/// Guarantees: the inner string is a valid StructFS path component (UAX#31
/// identifier or pure numeric). Cannot be constructed from an arbitrary
/// string without validation, which is what lets the `path!` macro accept
/// `PathComponent` expressions without a runtime check.
///
/// # Arbitrary strings
///
/// Real-world identifiers (`my-account`, `hello world`, UUIDs with dashes)
/// are often not valid components. Use [`PathComponent::encode`] to embed
/// them losslessly via [Namecode](https://crates.io/crates/namecode) and
/// [`PathComponent::decode`] to recover the original string.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PathComponent(String);

impl PathComponent {
    /// Validate and wrap a string as a path component.
    pub fn try_new(s: impl Into<String>) -> Result<Self, PathError> {
        let s = s.into();
        Path::validate_component(&s, 0)?;
        Ok(Self(s))
    }

    /// Encode an arbitrary string as a valid path component.
    ///
    /// Valid UAX#31 identifiers pass through unchanged; everything else
    /// (punctuation, spaces, leading digits) is Namecode-encoded into a
    /// `_N_`-prefixed identifier. Always succeeds and is deterministic.
    /// Reverse with [`PathComponent::decode`].
    pub fn encode(s: &str) -> Self {
        let encoded = namecode::encode(s);
        debug_assert!(Path::validate_component(&encoded, 0).is_ok());
        Self(encoded)
    }

    /// Decode a component produced by [`PathComponent::encode`] back to the
    /// original string.
    ///
    /// Components that are not Namecode-encoded are returned unchanged
    /// (matching `encode`'s pass-through of valid identifiers). Returns an
    /// error only for a malformed `_N_`-prefixed component.
    pub fn decode(&self) -> Result<String, PathError> {
        match namecode::decode(&self.0) {
            Ok(decoded) => Ok(decoded),
            Err(namecode::DecodeError::NotEncoded) => Ok(self.0.clone()),
            Err(e) => Err(PathError::InvalidComponent {
                component: self.0.clone(),
                position: 0,
                message: format!("malformed namecode encoding: {}", e),
            }),
        }
    }

    /// Get the validated string.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Borrow the validated string.
    ///
    /// Used by the `path!` macro to enforce that only `PathComponent` values
    /// (not bare `String`/`&str`) are accepted as runtime path components.
    /// Named distinctly so no standard type matches.
    pub fn validated_str(&self) -> &str {
        &self.0
    }

    /// Consume and return the inner string.
    pub fn into_string(self) -> String {
        self.0
    }
}

impl AsRef<str> for PathComponent {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for PathComponent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

// Numeric indices are always valid components.
impl From<usize> for PathComponent {
    fn from(i: usize) -> Self {
        Self(i.to_string())
    }
}

impl From<u64> for PathComponent {
    fn from(i: u64) -> Self {
        Self(i.to_string())
    }
}

impl From<PathComponent> for Path {
    fn from(c: PathComponent) -> Self {
        Path {
            components: vec![c.into_string()],
        }
    }
}

impl FromIterator<PathComponent> for Path {
    fn from_iter<I: IntoIterator<Item = PathComponent>>(iter: I) -> Self {
        Path {
            components: iter.into_iter().map(PathComponent::into_string).collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::path;

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
    fn macro_component_style() {
        let p = path!("users", 123, "name");
        assert_eq!(p.to_string(), "users/123/name");
        assert_eq!(p, path!("users/123/name"));
    }

    #[test]
    fn macro_empty() {
        let p = path!();
        assert!(p.is_empty());
    }

    #[test]
    fn macro_with_runtime_component() {
        let name = PathComponent::try_new("alice").unwrap();
        let p = path!("users", name, "profile");
        assert_eq!(p.to_string(), "users/alice/profile");
    }

    #[test]
    fn macro_mixed_literal_forms() {
        // A literal containing slashes can mix with separate components
        let p = path!("a/b", "c");
        assert_eq!(p.to_string(), "a/b/c");
    }

    #[test]
    fn path_component_validates() {
        assert!(PathComponent::try_new("accounts").is_ok());
        assert!(PathComponent::try_new("42").is_ok());
        assert!(PathComponent::try_new("café").is_ok());
        assert!(PathComponent::try_new("_private").is_ok());
        assert!(PathComponent::try_new("").is_err());
        assert!(PathComponent::try_new("my-account").is_err());
        assert!(PathComponent::try_new("my account").is_err());
        assert!(PathComponent::try_new(".hidden").is_err());
        assert!(PathComponent::try_new("_").is_err());
        assert!(PathComponent::try_new("a/b").is_err());
    }

    #[test]
    fn path_component_encode_roundtrip() {
        for original in [
            "plain",
            "my-account",
            "hello world",
            "slashes/and spaces",
            "oxide-🦀",
            "123-456",
        ] {
            let component = PathComponent::encode(original);
            // Encoded form is a valid component usable in paths
            assert!(PathComponent::try_new(component.as_str()).is_ok());
            assert_eq!(component.decode().unwrap(), original);
        }
    }

    #[test]
    fn path_component_encode_passthrough() {
        // Valid identifiers pass through unchanged
        let component = PathComponent::encode("plain");
        assert_eq!(component.as_str(), "plain");
        assert_eq!(component.decode().unwrap(), "plain");
    }

    #[test]
    fn path_component_from_index() {
        let c: PathComponent = 7usize.into();
        assert_eq!(c.as_str(), "7");
        let c: PathComponent = 7u64.into();
        assert_eq!(c.as_str(), "7");
    }

    #[test]
    fn child_and_push() {
        let base = path!("users");
        let p = base.child(PathComponent::try_new("alice").unwrap());
        assert_eq!(p.to_string(), "users/alice");

        let mut p2 = path!("items");
        p2.push(3usize);
        assert_eq!(p2.to_string(), "items/3");
    }

    #[test]
    fn path_from_component_iter() {
        let p: Path = ["a", "b", "c"]
            .iter()
            .map(|s| PathComponent::try_new(*s).unwrap())
            .collect();
        assert_eq!(p.to_string(), "a/b/c");
    }

    #[test]
    fn validate_component_public() {
        assert!(Path::validate_component("foo", 0).is_ok());
        let err = Path::validate_component("bad-name", 2).unwrap_err();
        assert!(err.to_string().contains("position 2"));
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
