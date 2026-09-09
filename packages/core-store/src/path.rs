//! Path type with validated Unicode identifier components.

use std::fmt;

use bytes::Bytes;
use structfs_ll_store::LLPath;

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
///
/// # Refinement of [`LLPath`]
///
/// `Path` is a validated *refinement* of the low-level [`LLPath`]: it wraps an
/// `LLPath` whose every component is additionally guaranteed to be valid UTF-8
/// and a valid component grammar (identifier or numeric). Because a `Path`
/// *is* an `LLPath` that has been validated, widening ([`as_ll`](Self::as_ll) /
/// [`into_ll`](Self::into_ll)) is free, and narrowing
/// ([`validate`](Self::validate)) is the single place validation happens.
/// Components are stored as byte components, so `Path -> LLPath` never copies
/// and structural ops (`join`/`slice`/`strip_prefix`) clone `Bytes`
/// (reference-count bumps) rather than deep-copying `String`s.
#[derive(Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub struct Path(LLPath);

/// View a validated byte component as `&str`.
///
/// Sound because the `Path` invariant guarantees every component is valid
/// UTF-8; this is the accessor that lets the high-level API keep speaking
/// `&str` over a byte representation without re-validating.
#[inline]
fn component_str(component: &Bytes) -> &str {
    // SAFETY: every component of a `Path` was validated as a UTF-8 identifier
    // or numeric string at construction (see `validate_component`).
    unsafe { std::str::from_utf8_unchecked(component) }
}

/// Build validated byte components from already-validated strings, moving the
/// `String` buffers into `Bytes` without copying.
fn ll_from_strings(components: Vec<String>) -> LLPath {
    components
        .into_iter()
        .map(|s| Bytes::from(s.into_bytes()))
        .collect()
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
            return Ok(Path(LLPath::new()));
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

        Ok(Path(ll_from_strings(components)))
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
        Path(ll_from_strings(components))
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
        Path(ll_from_strings(components))
    }

    /// Try to create a path from components, validating each.
    pub fn try_from_components(components: Vec<String>) -> Result<Self, PathError> {
        for (i, component) in components.iter().enumerate() {
            Self::validate_component(component, i)?;
        }
        Ok(Path(ll_from_strings(components)))
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
        self.0.is_empty()
    }

    /// Get the number of components.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Iterate over components as validated `&str`s.
    pub fn iter(&self) -> impl Iterator<Item = &str> {
        self.0.iter().map(component_str)
    }

    /// Join this path with another.
    #[must_use]
    pub fn join(&self, other: &Path) -> Path {
        let mut components = self.0.components().to_vec();
        components.extend(other.0.iter().cloned());
        Path(LLPath::from_components(components))
    }

    /// Return a new path with the component appended.
    #[must_use]
    pub fn child(&self, component: impl Into<PathComponent>) -> Path {
        let mut components = self.0.components().to_vec();
        components.push(Bytes::from(component.into().into_string().into_bytes()));
        Path(LLPath::from_components(components))
    }

    /// Append a component in place.
    pub fn push(&mut self, component: impl Into<PathComponent>) {
        self.0
            .push(Bytes::from(component.into().into_string().into_bytes()));
    }

    /// Check if this path has the given prefix.
    pub fn has_prefix(&self, prefix: &Path) -> bool {
        prefix.0.len() <= self.0.len()
            && prefix.0.components() == &self.0.components()[..prefix.0.len()]
    }

    /// Strip a prefix from this path.
    ///
    /// Returns `None` if the prefix doesn't match.
    #[must_use]
    pub fn strip_prefix(&self, prefix: &Path) -> Option<Path> {
        if self.has_prefix(prefix) {
            Some(Path(LLPath::from_components(
                self.0.components()[prefix.0.len()..].to_vec(),
            )))
        } else {
            None
        }
    }

    /// Get a slice of components as a new path.
    pub fn slice(&self, start: usize, end: usize) -> Path {
        Path(LLPath::from_components(
            self.0.components()[start..end].to_vec(),
        ))
    }

    /// Borrow this path as its underlying [`LLPath`] — the free widening from
    /// the validated high-level contract to the opaque low-level one.
    pub fn as_ll(&self) -> &LLPath {
        &self.0
    }

    /// Consume this path into its underlying [`LLPath`] — free widening with no
    /// component copy.
    pub fn into_ll(self) -> LLPath {
        self.0
    }

    /// Validate an [`LLPath`] into a `Path` — the single narrowing point where
    /// opaque bytes become a validated identifier path. Reuses the `Bytes`
    /// components (no copy); fails if any component is not valid UTF-8 or not a
    /// valid component grammar.
    pub fn validate(ll: LLPath) -> Result<Self, PathError> {
        for (i, component) in ll.iter().enumerate() {
            let s = std::str::from_utf8(component.as_ref()).map_err(|_| {
                PathError::InvalidComponent {
                    component: format!("{:?}", component.as_ref()),
                    position: i,
                    message: "not valid UTF-8".to_string(),
                }
            })?;
            Self::validate_component(s, i)?;
        }
        Ok(Path(ll))
    }

    /// Wrap an [`LLPath`] known to already satisfy the `Path` invariant, without
    /// re-validating. This is the byte-path analogue of
    /// [`from_validated_components`](Self::from_validated_components): use it for
    /// paths that originated host-side from a `Path` (e.g. a write result path
    /// echoed back), so internal LL->HL hops don't re-pay validation. Debug
    /// builds re-check as a safety net.
    pub fn from_ll_unchecked(ll: LLPath) -> Self {
        #[cfg(debug_assertions)]
        for (i, component) in ll.iter().enumerate() {
            let s = std::str::from_utf8(component.as_ref())
                .expect("pre-validated LL component is not UTF-8");
            Self::validate_component(s, i).expect("invalid pre-validated LL component");
        }
        Path(ll)
    }

    /// Convert to an owned LL path (byte components).
    ///
    /// Now a cheap clone (each component is a reference-counted `Bytes`); prefer
    /// [`as_ll`](Self::as_ll)/[`into_ll`](Self::into_ll) to avoid even that.
    pub fn to_ll_path(&self) -> LLPath {
        self.0.clone()
    }

    /// Try to create from borrowed LL path components (byte slices).
    ///
    /// Copies the components into owned `Bytes` and validates. Fails if any
    /// component is not valid UTF-8 or not a valid identifier. For an owned
    /// [`LLPath`], prefer [`validate`](Self::validate) to reuse its `Bytes`.
    pub fn try_from_ll_path(ll_path: &[impl AsRef<[u8]>]) -> Result<Self, PathError> {
        let ll: LLPath = ll_path
            .iter()
            .map(|b| Bytes::copy_from_slice(b.as_ref()))
            .collect();
        Self::validate(ll)
    }
}

impl fmt::Display for Path {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut first = true;
        for component in self.iter() {
            if !first {
                f.write_str("/")?;
            }
            f.write_str(component)?;
            first = false;
        }
        Ok(())
    }
}

impl std::ops::Index<usize> for Path {
    type Output = str;

    fn index(&self, i: usize) -> &Self::Output {
        component_str(&self.0[i])
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
        Path(LLPath::from_components(vec![Bytes::from(
            c.into_string().into_bytes(),
        )]))
    }
}

impl FromIterator<PathComponent> for Path {
    fn from_iter<I: IntoIterator<Item = PathComponent>>(iter: I) -> Self {
        Path(
            iter.into_iter()
                .map(|c| Bytes::from(c.into_string().into_bytes()))
                .collect(),
        )
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
        let components: Vec<&str> = p.iter().collect();
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

    #[test]
    fn as_ll_and_into_ll_widen_losslessly() {
        let p = path!("users/123/name");
        // Borrowing widening exposes the byte components in order.
        let ll = p.as_ll();
        assert_eq!(ll.len(), 3);
        assert_eq!(ll[0].as_ref(), b"users");
        assert_eq!(ll[2].as_ref(), b"name");
        // Owned widening yields the same components.
        assert_eq!(p.clone().into_ll(), ll.clone());
    }

    #[test]
    fn validate_narrows_and_rejects() {
        // A valid LLPath narrows to the equivalent Path.
        let ll: LLPath = [Bytes::from_static(b"a"), Bytes::from_static(b"b")]
            .into_iter()
            .collect();
        assert_eq!(Path::validate(ll).unwrap(), path!("a/b"));

        // A non-identifier component is rejected.
        let bad: LLPath = [Bytes::from_static(b"a-b")].into_iter().collect();
        assert!(Path::validate(bad).is_err());

        // Non-UTF-8 is rejected.
        let non_utf8: LLPath = [Bytes::from_static(&[0xff, 0xfe])].into_iter().collect();
        assert!(Path::validate(non_utf8).is_err());
    }

    #[test]
    fn widen_then_narrow_roundtrips() {
        let p = path!("users/名前/0");
        // Path -> LLPath (free) -> Path (validated) is the identity.
        assert_eq!(Path::validate(p.clone().into_ll()).unwrap(), p);
        // The trusted constructor agrees on already-valid input.
        assert_eq!(Path::from_ll_unchecked(p.clone().into_ll()), p);
    }
}
