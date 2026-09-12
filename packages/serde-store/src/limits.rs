//! Shared finite limits for wire codecs and typed conversion.
use std::fmt;
use structfs_core_store::{CodecErrorKind as Kind, CodecOperation, Error, Format, Value};

/// Inclusive bounds. Work counts traversed nodes and bytes, plus collection sorting
/// estimates. Allocation counts conservative reservations, not allocator metadata.
#[derive(Debug, Clone)]
pub struct Limits {
    pub max_input_bytes: usize,
    pub max_output_bytes: usize,
    pub max_depth: usize,
    pub max_nodes: usize,
    pub max_collection_entries: usize,
    pub max_string_bytes: usize,
    pub max_blob_bytes: usize,
    pub max_payload_bytes: usize,
    pub max_allocation_bytes: usize,
    pub max_work: usize,
    pub max_diagnostic_bytes: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            max_input_bytes: 16 << 20,
            max_output_bytes: 16 << 20,
            max_depth: 64,
            max_nodes: 262144,
            max_collection_entries: 65536,
            max_string_bytes: 4 << 20,
            max_blob_bytes: 8 << 20,
            max_payload_bytes: 16 << 20,
            max_allocation_bytes: 64 << 20,
            max_work: 128 << 20,
            max_diagnostic_bytes: 256,
        }
    }
}

#[derive(Debug)]
pub(crate) struct Failure(pub Kind);
pub(crate) type Result<T> = std::result::Result<T, Failure>;
impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self.0)
    }
}
impl std::error::Error for Failure {}
impl serde::ser::Error for Failure {
    fn custom<T: fmt::Display>(_: T) -> Self {
        Self(Kind::TypeMismatch)
    }
}
impl serde::de::Error for Failure {
    fn custom<T: fmt::Display>(_: T) -> Self {
        Self(Kind::TypeMismatch)
    }
}
impl Failure {
    pub(crate) fn core(self, format: &Format, operation: CodecOperation, limits: &Limits) -> Error {
        let mut message = format!("{:?}", self.0);
        message.truncate(limits.max_diagnostic_bytes.min(message.len()));
        Error::Codec {
            kind: self.0,
            operation,
            format: format.clone(),
            message,
        }
    }
}
pub(crate) fn ensure(ok: bool, kind: Kind) -> Result<()> {
    if ok {
        Ok(())
    } else {
        Err(Failure(kind))
    }
}
pub(crate) struct Budget<'a> {
    pub limits: &'a Limits,
    nodes: usize,
    payload: usize,
    allocation: usize,
    work: usize,
}
fn add(counter: &mut usize, n: usize, max: usize) -> Result<()> {
    *counter = counter.checked_add(n).ok_or(Failure(Kind::ResourceLimit))?;
    ensure(*counter <= max, Kind::ResourceLimit)
}
impl<'a> Budget<'a> {
    pub fn new(limits: &'a Limits) -> Self {
        Self {
            limits,
            nodes: 0,
            payload: 0,
            allocation: 0,
            work: 0,
        }
    }
    pub fn work(&mut self, n: usize) -> Result<()> {
        add(&mut self.work, n, self.limits.max_work)
    }
    pub fn allocate(&mut self, n: usize) -> Result<()> {
        add(&mut self.allocation, n, self.limits.max_allocation_bytes)
    }
    pub fn node(&mut self, depth: usize) -> Result<()> {
        ensure(depth <= self.limits.max_depth.min(256), Kind::ResourceLimit)?;
        add(&mut self.nodes, 1, self.limits.max_nodes)?;
        self.allocate(128)?;
        self.work(1)
    }
    pub fn entries(&mut self, n: usize) -> Result<()> {
        ensure(n <= self.limits.max_collection_entries, Kind::ResourceLimit)?;
        self.work(1)
    }
    pub fn payload(&mut self, n: usize, blob: bool) -> Result<()> {
        ensure(
            n <= if blob {
                self.limits.max_blob_bytes
            } else {
                self.limits.max_string_bytes
            },
            Kind::ResourceLimit,
        )?;
        add(&mut self.payload, n, self.limits.max_payload_bytes)?;
        self.allocate(n.saturating_mul(2))?;
        self.work(n)
    }
    pub fn key_work(&mut self, bytes: usize, entries: usize) -> Result<()> {
        // Conservative comparison estimate for BTreeMap lookup/insertion and
        // builder sorting. Long shared prefixes must not be free work.
        let comparisons = (usize::BITS - entries.max(1).leading_zeros()) as usize;
        self.work(bytes.saturating_mul(comparisons).saturating_mul(16))
    }
    pub fn tree(&mut self, v: &Value, depth: usize) -> Result<()> {
        self.node(depth)?;
        match v {
            Value::String(s) => self.payload(s.len(), false)?,
            Value::Bytes(b) => self.payload(b.len(), true)?,
            Value::Array(a) => {
                self.entries(a.len())?;
                for v in a {
                    self.tree(v, depth + 1)?;
                }
            }
            Value::Map(m) => {
                self.entries(m.len())?;
                for (k, v) in m {
                    self.payload(k.len(), false)?;
                    self.key_work(k.len(), m.len())?;
                    self.tree(v, depth + 1)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
}

/// Validate an existing value before traversing or cloning it at a trust boundary.
pub fn validate_value(value: &Value, limits: &Limits) -> std::result::Result<(), Error> {
    Budget::new(limits)
        .tree(value, 0)
        .map_err(|e| e.core(&Format::VALUE, CodecOperation::Encode, limits))
}
