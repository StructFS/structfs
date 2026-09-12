use crate::limits::{ensure, Budget, Failure, Limits, Result};
use std::collections::BTreeMap;
use structfs_core_store::{CodecErrorKind as K, Value};
struct Decoder<'a, 'b> {
    bytes: &'a [u8],
    budget: Budget<'b>,
}
impl<'a> Decoder<'a, '_> {
    fn range(&self, p: usize, n: usize) -> Result<&'a [u8]> {
        self.bytes
            .get(p..p.checked_add(n).ok_or(Failure(K::Syntax))?)
            .ok_or(Failure(K::Syntax))
    }
    fn uint(&self, p: usize, w: usize) -> Result<u64> {
        ensure(matches!(w, 1 | 2 | 4 | 8), K::Syntax)?;
        let mut b = [0; 8];
        b[..w].copy_from_slice(self.range(p, w)?);
        Ok(u64::from_le_bytes(b))
    }
    fn size(&self, p: usize, w: usize) -> Result<usize> {
        self.uint(p, w)?
            .try_into()
            .map_err(|_| Failure(K::ResourceLimit))
    }
    fn back(&self, p: usize, n: usize) -> Result<usize> {
        p.checked_sub(n).ok_or(Failure(K::Syntax))
    }
    fn offset(&self, p: usize, w: usize) -> Result<usize> {
        let n = self.size(p, w)?;
        self.back(p, n)
    }
    fn key(&mut self, p: usize, end: usize) -> Result<String> {
        let rest = self.bytes.get(p..end).ok_or(Failure(K::Syntax))?;
        let max = self.budget.limits.max_string_bytes;
        let n = rest
            .iter()
            .take(max.saturating_add(1))
            .position(|b| *b == 0)
            .ok_or(Failure(if rest.len() > max {
                K::ResourceLimit
            } else {
                K::Syntax
            }))?;
        self.budget.payload(n, false)?;
        Ok(std::str::from_utf8(&rest[..n])
            .map_err(|_| Failure(K::InvalidUnicode))?
            .into())
    }
    fn value(&mut self, p: usize, parent: usize, packed: u8, depth: usize) -> Result<Value> {
        self.budget.node(depth)?;
        ensure(depth <= 256, K::ResourceLimit)?;
        let t = packed >> 2;
        let width = 1usize << (packed & 3);
        ensure(matches!(t,0..=14|16..=26|36), K::UnsupportedValue)?;
        self.range(p, parent)?;
        let indirect = !matches!(t, 0..=3 | 26);
        if !indirect {
            ensure(width == parent, K::Syntax)?;
        }
        let addr = if indirect { self.offset(p, parent)? } else { p };
        let w = if indirect { width } else { parent };
        if matches!(t, 6..=8) {
            ensure(addr.checked_add(w).is_some_and(|end| end <= p), K::Syntax)?;
        }
        Ok(match t {
            0 => Value::Null,
            1 | 6 => {
                let n = self.uint(addr, w)?;
                let shift = (8 - w) * 8;
                Value::Integer(((n << shift) as i64) >> shift)
            }
            2 | 7 => Value::from(self.uint(addr, w)?),
            3 | 8 => {
                ensure(matches!(w, 4 | 8), K::Syntax)?;
                let n = self.uint(addr, w)?;
                Value::from(if w == 4 {
                    f32::from_bits(n as u32) as f64
                } else {
                    f64::from_bits(n)
                })
            }
            26 => {
                let n = self.uint(addr, w)?;
                ensure(n <= 1, K::Syntax)?;
                Value::Bool(n != 0)
            }
            4 => Value::String(self.key(addr, p)?),
            5 | 25 => {
                let n = self.size(self.back(addr, w)?, w)?;
                self.budget.payload(n, t == 25)?;
                ensure(
                    addr.checked_add(n)
                        .and_then(|end| end.checked_add(usize::from(t == 5)))
                        .is_some_and(|end| end <= p),
                    K::Syntax,
                )?;
                let b = self.range(addr, n)?;
                if t == 25 {
                    Value::Bytes(b.into())
                } else {
                    ensure(
                        self.range(addr.checked_add(n).ok_or(Failure(K::Syntax))?, 1)? == [0],
                        K::Syntax,
                    )?;
                    Value::String(
                        std::str::from_utf8(b)
                            .map_err(|_| Failure(K::InvalidUnicode))?
                            .into(),
                    )
                }
            }
            _ => {
                let fixed = matches!(t, 16..=24);
                let len = if fixed {
                    ((t - 16) / 3 + 2) as usize
                } else {
                    self.size(self.back(addr, w)?, w)?
                };
                self.budget.entries(len)?;
                let span = len.checked_mul(w).ok_or(Failure(K::Syntax))?;
                self.range(addr, span)?;
                let types = addr.checked_add(span).ok_or(Failure(K::Syntax))?;
                ensure(
                    types
                        .checked_add(if matches!(t, 9 | 10) { len } else { 0 })
                        .is_some_and(|end| end <= p),
                    K::Syntax,
                )?;
                if matches!(t, 9 | 10) {
                    self.range(types, len)?;
                }
                let element_type = match t {
                    11 => 1,
                    12 => 2,
                    13 => 3,
                    14 => 4,
                    36 => 26,
                    16..=24 => (t - 16) % 3 + 1,
                    _ => 0,
                };
                if t == 9 {
                    let meta = self.back(addr, 3 * w)?;
                    let keys = self.offset(meta, w)?;
                    let kw = self.size(meta + w, w)?;
                    ensure(matches!(kw, 1 | 2 | 4 | 8), K::Syntax)?;
                    ensure(self.size(self.back(keys, kw)?, kw)? == len, K::Syntax)?;
                    let key_span = len.checked_mul(kw).ok_or(Failure(K::Syntax))?;
                    ensure(
                        keys.checked_add(key_span).is_some_and(|end| end <= meta),
                        K::Syntax,
                    )?;
                    self.range(keys, key_span)?;
                    let mut map = BTreeMap::new();
                    let mut previous: Option<String> = None;
                    for i in 0..len {
                        let k = self.key(self.offset(keys + i * kw, kw)?, keys + i * kw)?;
                        if let Some(prev) = &previous {
                            ensure(prev != &k, K::DuplicateKey)?;
                            ensure(prev < &k, K::Syntax)?;
                        }
                        self.budget.key_work(k.len(), len)?;
                        self.budget.allocate(k.len())?;
                        previous = Some(k.clone());
                        let v =
                            self.value(addr + i * w, w, self.range(types + i, 1)?[0], depth + 1)?;
                        map.insert(k, v);
                    }
                    Value::Map(map)
                } else {
                    let mut a = Vec::new();
                    for i in 0..len {
                        let pack = if t == 10 {
                            self.range(types + i, 1)?[0]
                        } else {
                            (element_type << 2) | (packed & 3)
                        };
                        a.push(self.value(addr + i * w, w, pack, depth + 1)?);
                    }
                    Value::Array(a)
                }
            }
        })
    }
}
pub(crate) fn decode(bytes: &[u8], limits: &Limits) -> Result<Value> {
    ensure(bytes.len() <= limits.max_input_bytes, K::ResourceLimit)?;
    ensure(bytes.len() >= 3, K::Syntax)?;
    let n = bytes.len();
    let w = bytes[n - 1] as usize;
    ensure(matches!(w, 1 | 2 | 4 | 8), K::Syntax)?;
    let mut d = Decoder {
        bytes,
        budget: Budget::new(limits),
    };
    d.budget.work(n)?;
    d.value(
        (n - 2).checked_sub(w).ok_or(Failure(K::Syntax))?,
        w,
        bytes[n - 2],
        0,
    )
}
fn check(v: &Value, b: &mut Budget<'_>) -> Result<()> {
    // Reserve a conservative bound for the builder's buffer and temporary stacks.
    b.allocate(256)?;
    match v {
        Value::Map(m) => {
            for (k, v) in m {
                ensure(!k.contains('\0'), K::UnsupportedValue)?;
                b.allocate(k.len().saturating_mul(4))?;
                check(v, b)?;
            }
        }
        Value::Array(a) => {
            for v in a {
                check(v, b)?;
            }
        }
        Value::String(s) => b.allocate(s.len().saturating_mul(4))?,
        Value::Bytes(s) => b.allocate(s.len().saturating_mul(4))?,
        _ => {}
    }
    Ok(())
}
pub(crate) fn encode(v: &Value, limits: &Limits) -> Result<Vec<u8>> {
    let mut b = Budget::new(limits);
    b.tree(v, 0)?;
    check(v, &mut b)?;
    let mut normalized = v.clone();
    normalized.normalize();
    let bytes = flexbuffers::to_vec(&normalized).map_err(|_| Failure(K::UnsupportedValue))?;
    ensure(bytes.len() <= limits.max_output_bytes, K::ResourceLimit)?;
    Ok(bytes)
}
