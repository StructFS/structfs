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
    /// Output diagnostic byte cap. Custom Serde messages are captured with a
    /// hard 1024-byte ceiling and locations with a 256-byte ceiling. Truncation
    /// preserves UTF-8; locations use at most a quarter of this output budget.
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

// Serde's Error::custom has no access to caller limits. Capture at most this
// hard ceiling during formatting, then apply the caller's smaller output cap.
const DIAGNOSTIC_CAPTURE_BYTES: usize = 1024;

#[derive(Debug)]
pub(crate) struct Failure {
    kind: Kind,
    message: String,
    location: String,
}
pub(crate) type Result<T> = std::result::Result<T, Failure>;

fn bounded(value: impl fmt::Display, max: usize) -> String {
    use fmt::Write;
    struct Output {
        text: String,
        max: usize,
    }
    impl fmt::Write for Output {
        fn write_str(&mut self, text: &str) -> fmt::Result {
            let mut n = text.len().min(self.max - self.text.len());
            while !text.is_char_boundary(n) {
                n -= 1;
            }
            self.text.push_str(&text[..n]);
            if n < text.len() {
                Err(fmt::Error)
            } else {
                Ok(())
            }
        }
    }
    let mut output = Output {
        text: String::new(),
        max,
    };
    let _ = write!(output, "{value}");
    output.text
}
impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if !self.location.is_empty() {
            write!(f, "{}: ", self.location)?;
        }
        if self.message.is_empty() {
            write!(f, "{:?}", self.kind)
        } else {
            f.write_str(&self.message)
        }
    }
}
impl std::error::Error for Failure {}
impl serde::ser::Error for Failure {
    fn custom<T: fmt::Display>(message: T) -> Self {
        Self::diagnostic(message)
    }
}
impl serde::de::Error for Failure {
    fn custom<T: fmt::Display>(message: T) -> Self {
        Self::diagnostic(message)
    }
}
impl Failure {
    pub(crate) fn new(kind: Kind) -> Self {
        Self {
            kind,
            message: String::new(),
            location: String::new(),
        }
    }
    fn diagnostic(message: impl fmt::Display) -> Self {
        Self {
            kind: Kind::TypeMismatch,
            message: bounded(message, DIAGNOSTIC_CAPTURE_BYTES),
            location: String::new(),
        }
    }
    pub(crate) fn at(mut self, component: impl fmt::Display) -> Self {
        self.location = bounded(format_args!("{component}{}", self.location), 256);
        self
    }
    pub(crate) fn core(
        mut self,
        format: &Format,
        operation: CodecOperation,
        limits: &Limits,
    ) -> Error {
        // Reserve room for the actual diagnostic even with a very long location.
        let location = bounded(&self.location, limits.max_diagnostic_bytes / 4);
        self.location.clear();
        let message = if location.is_empty() {
            bounded(format_args!("{}", self), limits.max_diagnostic_bytes)
        } else {
            bounded(
                format_args!("{location}: {}", self),
                limits.max_diagnostic_bytes,
            )
        };
        Error::Codec {
            kind: self.kind,
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
        Err(Failure::new(kind))
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
    *counter = counter
        .checked_add(n)
        .ok_or(Failure::new(Kind::ResourceLimit))?;
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
