//! Path conversion utilities between legacy and new path types.
//!
//! Both path types have the same structure (`Vec<String>` components) and
//! very similar validation rules. The main difference is that the new path
//! uses `unicode-ident` for validation while the legacy uses regex.

use crate::Error;

/// Convert a legacy path to a new core-store path.
///
/// This should succeed for all valid legacy paths, since the new path
/// validation is a superset of the legacy validation.
pub fn legacy_path_to_core(
    legacy: &structfs_store::Path,
) -> Result<structfs_core_store::Path, Error> {
    structfs_core_store::Path::try_from_components(legacy.components.clone())
        .map_err(|e| Error::PathConversion(e.to_string()))
}

/// Convert a new core-store path to a legacy path.
///
/// This should succeed for all valid core-store paths.
pub fn core_path_to_legacy(
    core: &structfs_core_store::Path,
) -> Result<structfs_store::Path, Error> {
    // Reconstruct the path string and parse it through the legacy parser
    let path_str = core.components.join("/");
    structfs_store::Path::parse(&path_str).map_err(|e| Error::PathConversion(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_to_core_roundtrips() {
        let cases = vec!["", "foo", "foo/bar", "foo/bar/baz", "users/123/name"];

        for case in cases {
            let legacy = structfs_store::Path::parse(case).unwrap();
            let core = legacy_path_to_core(&legacy).unwrap();
            let back = core_path_to_legacy(&core).unwrap();
            assert_eq!(legacy, back, "roundtrip failed for: {}", case);
        }
    }

    #[test]
    fn core_to_legacy_roundtrips() {
        let cases = vec!["", "foo", "foo/bar", "foo/bar/baz", "items/0/value"];

        for case in cases {
            let core = structfs_core_store::Path::parse(case).unwrap();
            let legacy = core_path_to_legacy(&core).unwrap();
            let back = legacy_path_to_core(&legacy).unwrap();
            assert_eq!(core, back, "roundtrip failed for: {}", case);
        }
    }

    #[test]
    fn unicode_paths_convert() {
        let legacy =
            structfs_store::Path::parse("usuarios/名前").expect("legacy should support unicode");
        let core = legacy_path_to_core(&legacy).unwrap();
        assert_eq!(core.components, legacy.components);
    }
}
