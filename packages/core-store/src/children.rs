//! Names-only discovery as an ordinary read-address projection.
use crate::{Error, Path, Reader, Record, Value, Writer};

/// One page of child names, as returned by [`Reader::read_children_page`].
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct ChildPage {
    /// The names on this page, in provider order.
    pub names: Vec<String>,
    /// Cursor for the next page, or `None` when this page is the last.
    pub next: Option<usize>,
}

impl ChildPage {
    /// Build a page. `next` must be `None` on the final page and must be
    /// strictly greater than the requesting offset otherwise.
    pub fn new(names: Vec<String>, next: Option<usize>) -> Self {
        Self { names, next }
    }
}

/// Child names of a parsed value: map keys, or indices for arrays.
pub(crate) fn names_of_value(value: &Value) -> Vec<String> {
    match value {
        Value::Map(map) => map.keys().cloned().collect(),
        Value::Array(arr) => (0..arr.len()).map(|i| i.to_string()).collect(),
        _ => Vec::new(),
    }
}

/// Slice one page out of a fully materialized name list, validating the
/// cursor arguments the way every `read_children_page` implementation must.
pub(crate) fn page_names(
    names: Vec<String>,
    offset: usize,
    limit: usize,
) -> Result<ChildPage, Error> {
    if limit == 0 {
        return Err(Error::invalid_argument("child page limit must be positive"));
    }
    if offset > names.len() {
        return Err(Error::invalid_argument("child cursor past end"));
    }
    let end = offset.saturating_add(limit).min(names.len());
    let next = (end < names.len()).then_some(end);
    Ok(ChildPage::new(
        names.into_iter().skip(offset).take(limit).collect(),
        next,
    ))
}

/// Mount this read-only projection beside a data store. Read
/// `{offset}/{limit}/{target...}` to obtain `{names: [...], next: integer|null}`.
/// An absent target returns None; leaves and empty containers return an empty
/// page. Array names are indices. Offsets are provider cursors, not snapshots:
/// callers needing stable replay must enumerate an immutable/versioned store.
///
/// This forwards `Reader::read_children_page`, never `read` directly. Ordinary
/// async and service read adapters preserve the projection without a new verb.
/// Large providers must override paging: the default reader fallback enumerates
/// all names, bounding the response but not the provider's temporary memory.
pub struct ChildNames<S> {
    inner: S,
    max_names: usize,
    max_name_bytes: usize,
}
impl<S> ChildNames<S> {
    /// Wrap `inner`; both limits must be positive.
    pub fn new(inner: S, max_names: usize, max_name_bytes: usize) -> Result<Self, Error> {
        if max_names == 0 || max_name_bytes == 0 {
            return Err(Error::invalid_argument(
                "child page limits must be positive",
            ));
        }
        Ok(Self {
            inner,
            max_names,
            max_name_bytes,
        })
    }
    /// Unwrap, returning the inner store.
    pub fn into_inner(self) -> S {
        self.inner
    }
}
impl<S: Reader> Reader for ChildNames<S> {
    fn read(&mut self, from: &Path) -> Result<Option<Record>, Error> {
        if from.len() < 2 {
            return Err(Error::invalid_argument("expected offset/limit/target"));
        }
        let offset = from[0]
            .parse::<usize>()
            .map_err(|_| Error::invalid_argument("invalid child cursor"))?;
        let limit = from[1]
            .parse::<usize>()
            .map_err(|_| Error::invalid_argument("invalid child page limit"))?;
        if limit == 0 {
            return Err(Error::invalid_argument("child page limit must be positive"));
        }
        if limit > self.max_names {
            return Err(Error::resource_limit("child page item limit"));
        }
        let Some(page) =
            self.inner
                .read_children_page(&from.slice(2, from.len()), offset, limit)?
        else {
            return Ok(None);
        };
        if page.names.len() > limit
            || page
                .names
                .iter()
                .try_fold(0usize, |n, s| n.checked_add(s.len()))
                .is_none_or(|n| n > self.max_name_bytes)
        {
            return Err(Error::resource_limit(
                "child page exceeds configured limits",
            ));
        }
        if page.next.is_some_and(|n| n <= offset) {
            // The provider violated the paging contract; not a caller error.
            return Err(Error::store(
                "child_names",
                "read",
                "child cursor failed to advance",
            ));
        }
        let next = page
            .next
            .map(|n| i64::try_from(n).map(Value::Integer))
            .transpose()
            .map_err(|_| Error::resource_limit("child cursor exceeds wire range"))?
            .unwrap_or(Value::Null);
        Ok(Some(Record::parsed(Value::Map(
            [
                (
                    "names".into(),
                    Value::Array(page.names.into_iter().map(Value::String).collect()),
                ),
                ("next".into(), next),
            ]
            .into(),
        ))))
    }
}
impl<S: Send + Sync> Writer for ChildNames<S> {
    fn write(&mut self, _: &Path, _: Record) -> Result<Path, Error> {
        Err(Error::permission_denied(
            "child-name projection is read-only",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_names_validates_cursor_arguments() {
        let names = || vec!["a".to_string(), "b".to_string(), "c".to_string()];
        assert!(matches!(
            page_names(names(), 0, 0),
            Err(Error::InvalidArgument { .. })
        ));
        assert!(matches!(
            page_names(names(), 4, 1),
            Err(Error::InvalidArgument { .. })
        ));
        // An offset exactly at the end is an empty final page, not an error.
        assert_eq!(
            page_names(names(), 3, 1).unwrap(),
            ChildPage::new(vec![], None)
        );
        assert_eq!(
            page_names(names(), 1, 1).unwrap(),
            ChildPage::new(vec!["b".to_string()], Some(2))
        );
        assert_eq!(
            page_names(names(), 0, 10).unwrap(),
            ChildPage::new(names(), None)
        );
    }

    #[test]
    fn child_names_rejects_malformed_addresses_as_invalid_argument() {
        let mut projection = ChildNames::new(crate::MemoryStore::new(), 10, 100).unwrap();
        assert!(matches!(
            projection.read(&crate::path!("only_one")),
            Err(Error::InvalidArgument { .. })
        ));
        assert!(matches!(
            projection.read(&crate::path!("x/2/target")),
            Err(Error::InvalidArgument { .. })
        ));
        assert!(matches!(
            ChildNames::new(crate::MemoryStore::new(), 0, 1),
            Err(Error::InvalidArgument { .. })
        ));
    }
}
