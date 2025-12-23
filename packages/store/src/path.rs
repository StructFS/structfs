use std::fmt;

use lazy_static::lazy_static;
use regex::Regex;

use serde::de::Error as _;
use serde::Deserialize;
use serde::Deserializer;
use serde::Serialize;
use serde::Serializer;

static MAX_PATH_BYTES: usize = 4096; // Bytes
                                     //
struct PathEncoder {}

impl regex::Replacer for PathEncoder {
    fn replace_append(&mut self, captures: &regex::Captures<'_>, destination: &mut String) {
        if let Some(m) = captures.name("bad_code") {
            let encoded = format!(
                "__{}_",
                m.as_str()
                    .as_bytes()
                    .iter()
                    .map(|b| format!("{b:01x}"))
                    .collect::<String>()
            );
            destination.push_str(&encoded);
            return;
        }

        if captures.name("double_underscore").is_some() {
            destination.push_str("___");
            return;
        }

        let to_encode = captures.get(0).unwrap().as_str();
        destination.push_str(to_encode);
    }
}

struct PathDecoder {}

impl regex::Replacer for PathDecoder {
    fn replace_append(&mut self, captures: &regex::Captures<'_>, destination: &mut String) {
        if let Some(m) = captures.name("bad_code") {
            let encoded_bytes = m.as_str().as_bytes();
            // Remove prefix "__" and suffix "_"
            let without_delimiters = &encoded_bytes[2..encoded_bytes.len() - 1];
            if without_delimiters.len() % 2 != 0 {
                panic!("Values to decode are not an even pairing of hex nybbles");
            }

            let decoded_bytes: Vec<u8> = without_delimiters
                .chunks_exact(2)
                .map(|pair| {
                    let s: &str =
                        std::str::from_utf8(pair).expect("Bytes were not decodeable as UTF-8");
                    u8::from_str_radix(s, 16).expect("Failed to decode apparent hex pair")
                })
                .collect::<Vec<u8>>();
            let decoded_string =
                String::from_utf8(decoded_bytes).expect("Failed to decode component to utf8");
            destination.push_str(&decoded_string);
            return;
        }

        if captures.name("double_underscore").is_some() {
            destination.push_str("__");
            return;
        }

        let to_encode = captures.get(0).unwrap().as_str();
        destination.push_str(to_encode);
    }
}

#[derive(thiserror::Error, Debug, PartialEq, Eq)]
pub enum Error {
    #[error("The path is invalid {path:?}: {message}")]
    PathInvalid { path: Path, message: String },
    #[error("The path is invalid {path:?}: {message}")]
    PathStringInvalid { path: String, message: String },
    #[error("The path {path:?} was not writable: {message}")]
    PathNotWritable { path: Path, message: String },
    #[error("An unknown error occurred: {message}")]
    UnknownPathError { message: String },
}

/// A `Path` represents a field node in a 1rec record tree.
///
/// Colloquially, a "path" represents the collection of record nodes from some root to the target
/// field node.  A "path string" strictly represents an encoding of a "path" as a string
/// identifying such a path.  `Path.path` represents the parsed/valid path tokens, one per node in
/// the path.
#[derive(Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub struct Path {
    // TODO(alex): Figure out how to make static ref and owned versions of path so that we don't
    // always have to clone static &str.
    pub components: Vec<String>,
}

impl Serialize for Path {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&format!("{}", self))
    }
}

impl<'de> Deserialize<'de> for Path {
    fn deserialize<D>(deserializer: D) -> Result<Path, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s: String = Deserialize::deserialize(deserializer)?;

        Path::parse(&s).map_err(D::Error::custom)
    }
}

#[macro_export]
macro_rules! path {
    // TODO(alex): Figure out ways to verify this at compile time instead of exploding at runtime
    // for bad paths.
    ($path_string:expr) => {
        Path::parse($path_string).unwrap()
    };
    ($($args:tt),*) => {
        compile_error!("Expected 1 argument, got something else")
    };
}


impl Path {
    // TODO(alex): Provide an encode/decode facility to support any unicode-compatible string as a
    // path component.  People will want a way to encode "foo/bar/baz/.dotfile", for example.
    pub fn parse(path: &str) -> Result<Self, Error> {
        if path.is_empty() {
            return Ok(Path { components: vec![] });
        }

        Self::validate_path_length(path)?;

        // TODO(akesling): Provide validation errors in terms of the provided path instead of the
        // path derived from it.  Error messages based on paths aren't in terms of the mental model
        // a caller may be expecting when passing the path here.
        let result = Path {
            components: path
                .split('/')
                .map(std::borrow::ToOwned::to_owned)
                .collect::<Vec<String>>(),
        };
        Path::validate_path(&result)?;
        Ok(result)
    }

    // TODO(alex): Switch to using a Bootstring variant encoding instead of the hacky substitution
    // system provided now.
    // TODO(alex): Take iterable instead of slice.
    pub fn encode_from_arbitrary(components: &[&str]) -> Path {
        lazy_static! {
            static ref STRINGS_TO_TRANSLATE: Regex = Regex::new(
                r"(?x)
                        (?P<valid>
                            ^(((\p{XID_Start}\p{XID_Continue}*)|(_\p{XID_Continue}+))|([0-9]+))$
                        ) |
                        (?P<bad_code>[^\p{XID_Continue}_]+) |
                        (?P<double_underscore>__)
                    "
            )
            .unwrap();
        }

        Path {
            components: components
                .iter()
                .map(|arbitrary| {
                    // TODO(alex): Write a parser for this instead of overusing regular
                    // expressions.  Among other things, that should remove the need for multiple
                    // passes.
                    let encoded = STRINGS_TO_TRANSLATE
                        .replace_all(arbitrary, PathEncoder {})
                        .to_string();
                    let changed = encoded != *arbitrary;
                    (encoded, changed)
                })
                .map(|(encoded, changed)| {
                    // Add an e prefix if:
                    // 1) The string has changed, so we have an encoded value
                    // 2) The string hasn't changed, but:
                    // 2a) It starts with an e that we don't want to accidentally remove at
                    // decoding.
                    // 2b) It starts with a numeral, but the whole component is not a valid
                    // integer.  Note that integer components are "special" as they can represent
                    // indexes.  All other numeral-starting components are not considered valid.
                    if changed
                        || !encoded.is_empty()
                            && (&encoded[0..1] == "e"
                                || (encoded.chars().next().unwrap().is_numeric()
                                    && encoded.parse::<usize>().is_err()))
                    {
                        format!("e{}", encoded)
                    } else {
                        match encoded.as_str() {
                            // A single underscore is not a valid path component, so replace it with
                            // its encoding for simplicity.
                            "_" => "e__5f_".to_string(),
                            _ => encoded,
                        }
                    }
                })
                .collect(),
        }
    }

    /// Decodes a path (constructed from `encode_from_arbitrary`) back to the arbitrary components
    ///
    /// Only call if this path was created with `encode_from_arbitrary`.  May result in an Err if
    /// called on a valid path which contains sequences used to escape arbitrary encodings.
    ///
    /// Note that this _may_ succeed on "incompatible" input paths.  The resulting strings may
    /// include undesirable characters or be missing expected characters.
    pub fn decode_to_arbitrary(&self) -> Result<Vec<String>, Error> {
        lazy_static! {
            static ref STRINGS_TO_TRANSLATE: Regex = Regex::new(
                r"(?x)
                        (?P<double_underscore>___) |
                        (?P<bad_code>__[^_]+_)
                    "
            )
            .unwrap();
        }

        Ok(self
            .components
            .iter()
            .map(|arbitrary| {
                if !arbitrary.is_empty() && &arbitrary[0..1] == "e" {
                    // The encoding has an 'e' prefix (which is a single byte).  Let's skip that.
                    let without_prefix = &arbitrary[1..];
                    STRINGS_TO_TRANSLATE
                        .replace_all(without_prefix, PathDecoder {})
                        .to_string()
                } else {
                    arbitrary.to_string()
                }
            })
            .collect())
    }

    pub fn len(&self) -> usize {
        self.components.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn iter(&self) -> std::slice::Iter<String> {
        self.components.iter()
    }

    // TODO(alex): Take slice instead of start and end indexes
    // TODO(alex): Return Option<Path> / make this safe instead of panicking if end is out of
    // bounds.
    pub fn slice_as_path(&self, start: usize, end: usize) -> Path {
        let components = self.components[start..end]
            .iter()
            .map(|s| s.to_string())
            .collect();
        Path { components }
    }

    #[must_use]
    pub fn join(&self, suffix: &Path) -> Path {
        let mut new_path = self.components.clone();
        new_path.extend_from_slice(&suffix.components);

        Path {
            components: new_path,
        }
    }

    /// Tests if path has the given prefix
    pub fn has_prefix(&self, prefix: &Path) -> bool {
        prefix.components.len() <= self.components.len()
            && prefix.components == self.components[..prefix.components.len()]
    }

    /// Strips provided prefix and returns a new path.
    ///
    /// If the prefix was not found in this path, returns `None`.
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

    pub fn validate_path_length(path_string: &str) -> Result<(), Error> {
        if path_string.len() < MAX_PATH_BYTES {
            Ok(())
        } else {
            Err(Error::PathStringInvalid {
                path: path_string.to_string(),
                message: format!("Path length exceeds max of {} bytes", MAX_PATH_BYTES),
            })
        }
    }

    pub fn validate_path(p: &Path) -> Result<(), Error> {
        lazy_static! {
            static ref VALID_PATH_TOKEN_FORMAT: Regex =
                Regex::new(r"^(((\p{XID_Start}\p{XID_Continue}*)|(_\p{XID_Continue}+))|([0-9]+))$")
                    .unwrap();
        }

        for (i, path_component) in p.components.iter().enumerate() {
            if !VALID_PATH_TOKEN_FORMAT.is_match(path_component) {
                return Err(Error::PathInvalid {
                    path: p.clone(),
                    message: format!(
                        concat!(
                            "Path token at position #{} ({}) is not a valid identifier ",
                            "string (identified by [Unicode Standard Annex #31](",
                            "https://www.unicode.org/reports/tr31/tr31-33.html)."
                        ),
                        i, path_component
                    ),
                });
            }
        }

        Ok(())
    }
}

impl<'path> From<&'path Path> for &'path [String] {
    fn from(p: &'path Path) -> Self {
        &p.components
    }
}

#[cfg(test)]
mod path_tests {
    use super::*;

    #[test]
    fn token_path_validation_works() {
        assert_eq!(
            Path::validate_path(&Path {
                components: vec![
                    "مرحبا".to_string(),
                    "Ты".to_string(),
                    "ใหญ่".to_string(),
                    "美麗的".to_string(),
                    "العالمية".to_string()
                ],
            }),
            Ok(())
        );

        assert_eq!(
            Path::validate_path(&Path {
                components: vec!["foo_bar_".to_string(), "_baz".to_string(),],
            }),
            Ok(())
        );
    }

    #[test]
    fn simple_components_survive_encoding() {
        let input = "1/fo-_o/--___bar/34/%.baz/123/4/qux/quuz";
        let encoded = Path::encode_from_arbitrary(&input.split('/').collect::<Vec<&str>>());
        assert_eq!(encoded.slice_as_path(0, 1), path!("1"));
        assert_eq!(encoded.slice_as_path(3, 4), path!("34"));
        assert_eq!(encoded.slice_as_path(5, 7), path!("123/4"));
        assert_eq!(encoded.slice_as_path(7, 9), path!("qux/quuz"));
    }

    #[test]
    fn arbitrary_encoding_is_symmetric() {
        let input = "fo-_o/--___bar/%. baz/__/_/.";
        let encoded = Path::encode_from_arbitrary(&input.split('/').collect::<Vec<&str>>());
        let decoded = encoded
            .decode_to_arbitrary()
            .expect("Encoding could not be decoded");
        assert_eq!(decoded.join("/"), input);
    }

    #[test]
    fn arbitrary_encoding_does_not_support_empty_components() {
        let input = "fo-_o/--___bar/%. baz/__//.";
        let encoded = Path::encode_from_arbitrary(&input.split('/').collect::<Vec<&str>>());
        assert_eq!(
            Path::validate_path(&encoded),
            Err(Error::PathInvalid {
                path: encoded.clone(),
                message: concat!(
                    "Path token at position #4 () is not a valid identifier string ",
                    "(identified by [Unicode Standard Annex #31](",
                    "https://www.unicode.org/reports/tr31/tr31-33.html)."
                )
                .to_string(),
            })
        );
    }

    #[test]
    fn free_underscore_disallowed() {
        let test_path = Path {
            components: vec![
                "foo".to_string(),
                "bar".to_string(),
                "_".to_string(),
                "baz".to_string(),
            ],
        };
        assert_eq!(
            Path::validate_path(&test_path),
            Err(Error::PathInvalid {
                path: test_path.clone(),
                message: concat!(
                    "Path token at position #2 (_) is not a valid identifier string ",
                    "(identified by [Unicode Standard Annex #31](",
                    "https://www.unicode.org/reports/tr31/tr31-33.html)."
                )
                .to_string(),
            })
        );
    }

    #[test]
    fn dots_alone_disallowed() {
        let current_dir_path = Path {
            components: vec![
                "foo".to_string(),
                "bar".to_string(),
                ".".to_string(),
                "baz".to_string(),
            ],
        };
        assert_eq!(
            Path::validate_path(&current_dir_path),
            Err(Error::PathInvalid {
                path: current_dir_path.clone(),
                message: concat!(
                    "Path token at position #2 (.) is not a valid identifier string ",
                    "(identified by [Unicode Standard Annex #31](",
                    "https://www.unicode.org/reports/tr31/tr31-33.html)."
                )
                .to_string(),
            })
        );

        let parent_dir_path = Path {
            components: vec![
                "foo".to_string(),
                "bar".to_string(),
                "..".to_string(),
                "baz".to_string(),
            ],
        };
        assert_eq!(
            Path::validate_path(&parent_dir_path),
            Err(Error::PathInvalid {
                path: parent_dir_path.clone(),
                message: concat!(
                    "Path token at position #2 (..) is not a valid identifier string ",
                    "(identified by [Unicode Standard Annex #31](",
                    "https://www.unicode.org/reports/tr31/tr31-33.html)."
                )
                .to_string(),
            })
        );
    }

    #[test]
    fn spaces_disallowed() {
        let non_breaking_space_characters =
            vec![' ', '\u{00A0}', '\u{202F}', '\u{2007}', '\u{2060}'];
        for space in &non_breaking_space_characters {
            let bad_token = format!("foo{}bar", space);
            let test_path = Path {
                components: vec![bad_token.clone(), "bar".to_string(), "baz".to_string()],
            };
            assert_eq!(
                Path::validate_path(&test_path),
                Err(Error::PathInvalid {
                    path: test_path.clone(),
                    message: format!(
                        concat!(
                            "Path token at position #0 ({}) is not a valid identifier string ",
                            "(identified by [Unicode Standard Annex #31](",
                            "https://www.unicode.org/reports/tr31/tr31-33.html)."
                        ),
                        bad_token
                    )
                    .to_string(),
                })
            );
        }
    }

    #[test]
    fn has_prefix_works() {
        assert!(path!("").has_prefix(&path!("")));
        assert!(path!("foo/bar/baz").has_prefix(&path!("")));
        assert!(path!("foo/bar/baz").has_prefix(&path!("foo")));
        assert!(path!("foo/bar/baz").has_prefix(&path!("foo/bar")));
        assert!(path!("foo/bar/baz").has_prefix(&path!("foo/bar/baz")));

        assert!(!path!("foo/bar/baz").has_prefix(&path!("bar")));
        assert!(!path!("foo/bar/baz").has_prefix(&path!("foo/bar/baz/qux")));
        assert!(!path!("foo/bar/baz").has_prefix(&path!("qux")));
        assert!(!path!("hello").has_prefix(&path!("read")));
    }
}

impl fmt::Display for Path {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.components.join("/"))
    }
}

impl std::ops::Index<usize> for Path {
    type Output = String;

    fn index(&self, i: usize) -> &Self::Output {
        &self.components[i]
    }
}
