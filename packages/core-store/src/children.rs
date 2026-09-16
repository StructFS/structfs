//! Names-only discovery as an ordinary read-address projection.
use crate::{Error, Path, Reader, Record, Value, Writer};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChildPage {
    pub names: Vec<String>,
    pub next: Option<usize>,
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
    pub fn new(inner: S, max_names: usize, max_name_bytes: usize) -> Result<Self, Error> {
        if max_names == 0 || max_name_bytes == 0 {
            return Err(Error::conflict("child page limits must be positive"));
        }
        Ok(Self {
            inner,
            max_names,
            max_name_bytes,
        })
    }
    pub fn into_inner(self) -> S {
        self.inner
    }
}
impl<S: Reader> Reader for ChildNames<S> {
    fn read(&mut self, from: &Path) -> Result<Option<Record>, Error> {
        if from.len() < 2 {
            return Err(Error::conflict("expected offset/limit/target"));
        }
        let offset = from[0]
            .parse::<usize>()
            .map_err(|_| Error::conflict("invalid child cursor"))?;
        let limit = from[1]
            .parse::<usize>()
            .map_err(|_| Error::conflict("invalid child page limit"))?;
        if limit == 0 || limit > self.max_names {
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
            return Err(Error::conflict("child cursor failed to advance"));
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
