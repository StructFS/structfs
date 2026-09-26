//! Shared finite limits for wire codecs and typed conversion.
use std::fmt;
use structfs_core_store::{CodecErrorKind as Kind, CodecOperation, Error, Format, Value};

/// The absolute nesting ceiling, independent of any caller-configured
/// [`Limits::max_depth`]. It bounds recursion in the decoders, which use the
/// native stack, so no configuration can make a deep document overflow it.
pub(crate) const DEPTH_CEILING: usize = 256;

/// Inclusive bounds. Work counts traversed nodes and bytes, plus collection sorting
/// estimates. Allocation counts conservative reservations, not allocator metadata.
///
/// Build one by adjusting the default rather than by struct literal — the type
/// is `#[non_exhaustive]`, so new bounds can be added without a breaking
/// change:
///
/// ```
/// use structfs_serde_store::Limits;
///
/// let limits = Limits::default().with_max_depth(8).with_max_input_bytes(4096);
/// assert_eq!(limits.max_depth, 8);
/// ```
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Limits {
    /// Largest document accepted by a decoder.
    pub max_input_bytes: usize,
    /// Largest document produced by an encoder.
    pub max_output_bytes: usize,
    /// Deepest nesting accepted, counted in container levels. Values above a
    /// hard ceiling have no effect: the decoders recurse on the native stack
    /// and refuse anything deeper regardless of configuration. The ceiling
    /// is 256 for plain JSON, CBOR and FlexBuffers, and 84 for tagged Value
    /// JSON, which spends up to three syntax levels per value level; the
    /// tagged encoder refuses anything deeper, so it never writes a document
    /// its decoder cannot read.
    pub max_depth: usize,
    /// Most values traversed in one operation.
    pub max_nodes: usize,
    /// Most entries in any single array or map.
    pub max_collection_entries: usize,
    /// Longest single string, in bytes.
    pub max_string_bytes: usize,
    /// Longest single byte string.
    pub max_blob_bytes: usize,
    /// Total string and byte-string payload across one operation.
    pub max_payload_bytes: usize,
    /// Conservative reservation ceiling; not allocator metadata.
    pub max_allocation_bytes: usize,
    /// Abstract work ceiling: nodes and bytes traversed, plus collection
    /// sorting and key-comparison estimates.
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

/// One setter per bound, so `Limits::default().with_…(n)` replaces the struct
/// literal that `#[non_exhaustive]` no longer allows from outside the crate.
macro_rules! limit_setters {
    ($($setter:ident => $field:ident),* $(,)?) => {
        impl Limits {
            $(
                #[doc = concat!("Set [`Limits::", stringify!($field), "`].")]
                #[must_use]
                pub fn $setter(mut self, value: usize) -> Self {
                    self.$field = value;
                    self
                }
            )*
        }
    };
}
limit_setters! {
    with_max_input_bytes => max_input_bytes,
    with_max_output_bytes => max_output_bytes,
    with_max_depth => max_depth,
    with_max_nodes => max_nodes,
    with_max_collection_entries => max_collection_entries,
    with_max_string_bytes => max_string_bytes,
    with_max_blob_bytes => max_blob_bytes,
    with_max_payload_bytes => max_payload_bytes,
    with_max_allocation_bytes => max_allocation_bytes,
    with_max_work => max_work,
    with_max_diagnostic_bytes => max_diagnostic_bytes,
}

// Serde's Error::custom has no access to caller limits. Capture at most this
// hard ceiling during formatting, then apply the caller's smaller output cap.
const DIAGNOSTIC_CAPTURE_BYTES: usize = 1024;

#[derive(Debug)]
pub(crate) struct Failure {
    kind: Kind,
    message: String,
    /// Path components from outermost to innermost, kept unjoined. Joining on
    /// every [`Failure::at`] would rebuild the whole prefix once per level,
    /// which is quadratic in the nesting depth of the failing document.
    location: std::collections::VecDeque<String>,
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
/// Maximum location bytes rendered when no caller budget applies.
const LOCATION_BYTES: usize = 256;

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let location = self.location_string(LOCATION_BYTES);
        if !location.is_empty() {
            write!(f, "{location}: ")?;
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
            location: std::collections::VecDeque::new(),
        }
    }
    fn diagnostic(message: impl fmt::Display) -> Self {
        Self {
            kind: Kind::TypeMismatch,
            message: bounded(message, DIAGNOSTIC_CAPTURE_BYTES),
            location: std::collections::VecDeque::new(),
        }
    }
    /// Prepend one path component. `at` is called as the error unwinds, so the
    /// innermost component arrives first and the outermost last; the deque
    /// keeps them in rendering order at O(1) per level.
    pub(crate) fn at(mut self, component: impl fmt::Display) -> Self {
        if self.location.len() < DEPTH_CEILING {
            self.location.push_front(bounded(component, LOCATION_BYTES));
        }
        self
    }
    /// Join the location, spending at most `max` bytes. Outermost components
    /// win: they are the ones that locate the failure for a reader.
    fn location_string(&self, max: usize) -> String {
        let mut out = String::new();
        for part in &self.location {
            if out.len() >= max {
                break;
            }
            let mut n = part.len().min(max - out.len());
            while !part.is_char_boundary(n) {
                n -= 1;
            }
            out.push_str(&part[..n]);
        }
        out
    }
    pub(crate) fn core(
        mut self,
        format: &Format,
        operation: CodecOperation,
        limits: &Limits,
    ) -> Error {
        // Reserve room for the actual diagnostic even with a very long location.
        let location = self.location_string(limits.max_diagnostic_bytes / 4);
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
        ensure(
            depth <= self.limits.max_depth.min(DEPTH_CEILING),
            Kind::ResourceLimit,
        )?;
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
