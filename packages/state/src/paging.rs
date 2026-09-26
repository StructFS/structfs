//! Snapshot flattening and page assembly.
//!
//! Every page here is built by *accumulating* the encoded size of each item as
//! it is appended. The obvious alternative — encode the whole page after each
//! append and compare against the budget — is quadratic in the page length,
//! which is exactly the shape of work a remote caller gets to choose.

use std::collections::BTreeMap;

use structfs_core_store::{Path, Value};

use crate::limits::{limit, size, StateLimits, PAGE_OVERHEAD};
use crate::{Change, ChangePage, Fault, Node, ReadLimits, SnapshotPage, Token};

/// A snapshot node together with the canonical size it was measured at, so
/// paging never has to encode it again.
pub(crate) struct SizedNode {
    pub(crate) node: Node,
    pub(crate) bytes: usize,
}

/// Flatten a value into preorder nodes: containers carry empty skeletons,
/// leaves carry their value. Paths are relative to the snapshot prefix.
pub(crate) fn flatten(
    value: Option<&Value>,
    limits: &StateLimits,
) -> Result<Vec<SizedNode>, Fault> {
    fn visit(
        v: &Value,
        path: &mut Vec<String>,
        out: &mut Vec<SizedNode>,
        used: &mut usize,
        l: &StateLimits,
    ) -> Result<(), Fault> {
        if out.len() >= l.nodes {
            return Err(limit("snapshot nodes"));
        }
        if path.iter().map(String::len).sum::<usize>() > l.snapshot_bytes.saturating_sub(*used) {
            return Err(limit("snapshot path bytes"));
        }
        let skeleton = match v {
            Value::Map(_) => Value::Map(BTreeMap::new()),
            Value::Array(_) => Value::Array(vec![]),
            other => other.clone(),
        };
        let node = Node {
            path: path.clone(),
            value: skeleton,
        };
        let bytes = size(&node)?;
        if bytes > l.snapshot_bytes.saturating_sub(*used) {
            return Err(limit("snapshot bytes"));
        }
        *used += bytes;
        out.push(SizedNode { node, bytes });
        match v {
            Value::Map(m) => {
                for (k, v) in m {
                    path.push(k.clone());
                    visit(v, path, out, used, l)?;
                    path.pop();
                }
            }
            Value::Array(a) => {
                for (i, v) in a.iter().enumerate() {
                    path.push(i.to_string());
                    visit(v, path, out, used, l)?;
                    path.pop();
                }
            }
            _ => (),
        }
        Ok(())
    }
    let mut nodes = vec![];
    if let Some(v) = value {
        visit(v, &mut vec![], &mut nodes, &mut 0, limits)?;
    }
    Ok(nodes)
}

/// One page of a pinned snapshot, starting at `cursor`.
pub(crate) fn snapshot_page(
    token: &Token,
    nodes: &[SizedNode],
    cursor: u64,
    limits: &ReadLimits,
) -> Result<SnapshotPage, Fault> {
    let total = nodes.len() as u64;
    if cursor > total {
        return Err(invalid_cursor());
    }
    let mut page = SnapshotPage {
        token: token.clone(),
        items: vec![],
        next: cursor,
        done: cursor == total,
    };
    let mut bytes = PAGE_OVERHEAD;
    for sized in nodes.iter().skip(cursor as usize).take(limits.page_items) {
        if sized.bytes > limits.page_bytes.saturating_sub(bytes) {
            break;
        }
        bytes += sized.bytes;
        page.items.push(sized.node.clone());
        page.next += 1;
    }
    page.done = page.next == total;
    if page.items.is_empty() && !page.done {
        return Err(limit("snapshot page"));
    }
    Ok(page)
}

fn invalid_cursor() -> Fault {
    crate::limits::invalid("snapshot cursor")
}

/// A committed change projected onto one handle's prefix, or `None` when the
/// commit touched nothing that handle can see.
pub(crate) fn project_change(change: &Change, prefix: &Path) -> Option<Change> {
    let paths: Vec<String> = change
        .paths
        .iter()
        .filter_map(|p| {
            // Commit paths were built from validated `Path` values; a path
            // that no longer parses is dropped rather than trusted.
            let p = Path::parse(p).ok()?;
            if let Some(relative) = p.strip_prefix(prefix) {
                Some(relative.to_string())
            } else if prefix.strip_prefix(&p).is_some() {
                Some(String::new())
            } else {
                None
            }
        })
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    (!paths.is_empty()).then(|| Change {
        token: change.token.clone(),
        paths,
    })
}

/// Accumulates changes into a page under a byte and item budget.
pub(crate) struct ChangePageBuilder {
    page: ChangePage,
    bytes: usize,
    limits: ReadLimits,
}
impl ChangePageBuilder {
    pub(crate) fn new(from: Token, limits: ReadLimits) -> Self {
        Self {
            page: ChangePage {
                items: vec![],
                next: from,
                done: false,
            },
            bytes: PAGE_OVERHEAD,
            limits,
        }
    }
    /// Advance past a commit this handle cannot see, without emitting it.
    pub(crate) fn skip_to(&mut self, token: Token) {
        self.page.next = token;
    }
    /// Append a visible change. Returns false when the page is full, leaving
    /// the cursor where it was before the attempt.
    pub(crate) fn push(&mut self, change: Change) -> Result<bool, Fault> {
        if self.page.items.len() >= self.limits.page_items {
            return Ok(false);
        }
        let bytes = size(&change)?;
        if bytes > self.limits.page_bytes.saturating_sub(self.bytes) {
            return Ok(false);
        }
        self.bytes += bytes;
        self.page.next = change.token.clone();
        self.page.items.push(change);
        Ok(true)
    }
    pub(crate) fn cursor(&self) -> &Token {
        &self.page.next
    }
    pub(crate) fn finish(self) -> ChangePage {
        self.page
    }
}
