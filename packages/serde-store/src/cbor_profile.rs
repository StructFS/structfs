use crate::json_profile::Output;
use crate::limits::{ensure, Budget, Failure, Limits, Result};
use std::collections::BTreeMap;
use structfs_core_store::{CodecErrorKind as K, Value};
struct Decoder<'a, 'b> {
    bytes: &'a [u8],
    pos: usize,
    budget: Budget<'b>,
}
impl<'a> Decoder<'a, '_> {
    fn read(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self.pos.checked_add(n).ok_or(Failure(K::Syntax))?;
        let b = self.bytes.get(self.pos..end).ok_or(Failure(K::Syntax))?;
        self.pos = end;
        Ok(b)
    }
    fn head(&mut self) -> Result<(u8, u8, u64)> {
        let b = self.read(1)?[0];
        let ai = b & 31;
        if ai == 31 {
            return Err(Failure(K::UnsupportedValue));
        }
        let n = match ai {
            0..=23 => ai as u64,
            24..=27 => {
                let mut n = 0;
                for b in self.read(1 << (ai - 24))? {
                    n = (n << 8) | *b as u64;
                }
                n
            }
            _ => return Err(Failure(K::Syntax)),
        };
        Ok((b >> 5, ai, n))
    }
    fn text(&mut self, n: u64) -> Result<String> {
        let n = usize::try_from(n).map_err(|_| Failure(K::ResourceLimit))?;
        self.budget.payload(n, false)?;
        Ok(std::str::from_utf8(self.read(n)?)
            .map_err(|_| Failure(K::InvalidUnicode))?
            .into())
    }
    fn value(&mut self, depth: usize) -> Result<Value> {
        self.budget.node(depth)?;
        ensure(depth <= 256, K::ResourceLimit)?;
        let (major, ai, n) = self.head()?;
        Ok(match major {
            0 => Value::from(n),
            1 => {
                ensure(n <= i64::MAX as u64, K::OutOfRange)?;
                Value::Integer(-1 - (n as i64))
            }
            2 => {
                let n = usize::try_from(n).map_err(|_| Failure(K::ResourceLimit))?;
                self.budget.payload(n, true)?;
                Value::Bytes(self.read(n)?.into())
            }
            3 => Value::String(self.text(n)?),
            4 | 5 => {
                let len = usize::try_from(n).map_err(|_| Failure(K::ResourceLimit))?;
                self.budget.entries(len)?;
                if major == 4 {
                    let mut a = Vec::new();
                    for _ in 0..len {
                        a.push(self.value(depth + 1)?);
                    }
                    Value::Array(a)
                } else {
                    let mut m = BTreeMap::new();
                    for _ in 0..len {
                        let (t, _, n) = self.head()?;
                        ensure(t == 3, K::UnsupportedValue)?;
                        let k = self.text(n)?;
                        self.budget.key_work(k.len(), m.len() + 1)?;
                        ensure(!m.contains_key(&k), K::DuplicateKey)?;
                        m.insert(k, self.value(depth + 1)?);
                    }
                    Value::Map(m)
                }
            }
            7 => match ai {
                20 => Value::Bool(false),
                21 => Value::Bool(true),
                22 => Value::Null,
                25 => Value::from(half(n as u16)),
                26 => Value::from(f32::from_bits(n as u32) as f64),
                27 => Value::from(f64::from_bits(n)),
                24 if n < 32 => return Err(Failure(K::Syntax)),
                _ => return Err(Failure(K::UnsupportedValue)),
            },
            _ => return Err(Failure(K::UnsupportedValue)),
        })
    }
}
fn half(bits: u16) -> f64 {
    let sign = if bits & 0x8000 == 0 { 1.0 } else { -1.0 };
    let e = (bits >> 10) & 31;
    let f = bits & 1023;
    sign * match e {
        0 => (f as f64) * 2f64.powi(-24),
        31 => {
            if f == 0 {
                f64::INFINITY
            } else {
                f64::NAN
            }
        }
        _ => (1.0 + (f as f64) / 1024.0) * 2f64.powi(e as i32 - 15),
    }
}
pub(crate) fn decode(bytes: &[u8], limits: &Limits) -> Result<Value> {
    ensure(bytes.len() <= limits.max_input_bytes, K::ResourceLimit)?;
    let mut d = Decoder {
        bytes,
        pos: 0,
        budget: Budget::new(limits),
    };
    d.budget.work(bytes.len())?;
    let v = d.value(0)?;
    ensure(d.pos == bytes.len(), K::Syntax)?;
    Ok(v)
}
fn head(out: &mut Output<'_>, major: u8, n: u64) -> Result<()> {
    let w = if n < 24 {
        0
    } else if n <= 255 {
        1
    } else if n <= 65535 {
        2
    } else if n <= u32::MAX as u64 {
        4
    } else {
        8
    };
    out.put(&[(major << 5)
        | match w {
            0 => n as u8,
            1 => 24,
            2 => 25,
            4 => 26,
            _ => 27,
        }])?;
    if w > 0 {
        out.put(&n.to_be_bytes()[8 - w..])?;
    }
    Ok(())
}
fn value(v: &Value, out: &mut Output<'_>) -> Result<()> {
    match v {
        Value::Null => out.put(&[0xf6]),
        Value::Bool(x) => out.put(&[if *x { 0xf5 } else { 0xf4 }]),
        Value::Integer(n) => {
            if *n < 0 {
                head(out, 1, (-1 - *n) as u64)
            } else {
                head(out, 0, *n as u64)
            }
        }
        Value::Unsigned(n) => head(out, 0, *n),
        Value::Float(f) => {
            out.put(&[0xfb])?;
            out.put(
                &(if f.is_nan() {
                    0x7ff8000000000000
                } else {
                    f.to_bits()
                })
                .to_be_bytes(),
            )
        }
        Value::Bytes(b) => {
            head(out, 2, b.len() as u64)?;
            out.put(b)
        }
        Value::String(s) => {
            head(out, 3, s.len() as u64)?;
            out.put(s.as_bytes())
        }
        Value::Array(a) => {
            head(out, 4, a.len() as u64)?;
            for v in a {
                value(v, out)?;
            }
            Ok(())
        }
        Value::Map(m) => {
            head(out, 5, m.len() as u64)?;
            for (k, v) in m {
                head(out, 3, k.len() as u64)?;
                out.put(k.as_bytes())?;
                value(v, out)?;
            }
            Ok(())
        }
        _ => Err(Failure(K::UnsupportedValue)),
    }
}
pub(crate) fn encode(v: &Value, limits: &Limits) -> Result<Vec<u8>> {
    let mut out = Output::new(limits);
    out.budget.tree(v, 0)?;
    value(v, &mut out)?;
    Ok(out.bytes)
}
