use crate::limits::{ensure, Budget, Failure, Limits, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use std::collections::BTreeMap;
use structfs_core_store::{CodecErrorKind as K, Value};

enum Json<'a> {
    Null,
    Bool(bool),
    Number(&'a str),
    String(String),
    Array(Vec<Json<'a>>),
    Map(Vec<(String, Json<'a>)>),
}
struct Parser<'a, 'b> {
    input: &'a [u8],
    pos: usize,
    budget: Budget<'b>,
    tagged: bool,
}
impl<'a> Parser<'a, '_> {
    fn ws(&mut self) {
        while self
            .input
            .get(self.pos)
            .is_some_and(|b| matches!(b, b' ' | b'\n' | b'\r' | b'\t'))
        {
            self.pos += 1;
        }
    }
    fn take(&mut self, b: u8) -> Result<()> {
        self.ws();
        ensure(self.input.get(self.pos) == Some(&b), K::Syntax)?;
        self.pos += 1;
        Ok(())
    }
    fn string(&mut self) -> Result<String> {
        self.ws();
        let start = self.pos;
        self.take(b'"')?;
        loop {
            let b = *self.input.get(self.pos).ok_or(Failure(K::Syntax))?;
            self.pos += 1;
            if b == b'"' {
                break;
            }
            ensure(b >= 32, K::Syntax)?;
            if b == b'\\' {
                let b = *self.input.get(self.pos).ok_or(Failure(K::Syntax))?;
                self.pos += 1;
                if b == b'u' {
                    for _ in 0..4 {
                        ensure(
                            self.input.get(self.pos).is_some_and(u8::is_ascii_hexdigit),
                            K::Syntax,
                        )?;
                        self.pos += 1;
                    }
                } else {
                    ensure(
                        matches!(b, b'"' | b'\\' | b'/' | b'b' | b'f' | b'n' | b'r' | b't'),
                        K::Syntax,
                    )?;
                }
            }
        }
        self.budget.allocate((self.pos - start).saturating_mul(2))?;
        serde_json::from_slice(&self.input[start..self.pos]).map_err(|_| Failure(K::InvalidUnicode))
    }
    fn value(&mut self, depth: usize) -> Result<Json<'a>> {
        // A hard stack ceiling is independent of caller-configured semantic depth.
        let max = if self.tagged {
            self.budget
                .limits
                .max_depth
                .saturating_mul(3)
                .saturating_add(8)
        } else {
            self.budget.limits.max_depth
        };
        ensure(depth <= max.min(256), K::ResourceLimit)?;
        self.budget.allocate(128)?;
        self.budget.work(1)?;
        self.ws();
        match self
            .input
            .get(self.pos)
            .copied()
            .ok_or(Failure(K::Syntax))?
        {
            b'n' => {
                self.literal(b"null")?;
                Ok(Json::Null)
            }
            b't' => {
                self.literal(b"true")?;
                Ok(Json::Bool(true))
            }
            b'f' => {
                self.literal(b"false")?;
                Ok(Json::Bool(false))
            }
            b'"' => self.string().map(Json::String),
            b'[' | b'{' => {
                let map = self.input[self.pos] == b'{';
                self.pos += 1;
                self.ws();
                let close = if map { b'}' } else { b']' };
                let mut a = Vec::new();
                let mut m = Vec::new();
                if self.input.get(self.pos) == Some(&close) {
                    self.pos += 1;
                    return Ok(if map { Json::Map(m) } else { Json::Array(a) });
                }
                loop {
                    let count = if map { m.len() } else { a.len() };
                    // Syntax arrays include fixed envelope/entry wrappers.
                    ensure(
                        count
                            < self
                                .budget
                                .limits
                                .max_collection_entries
                                .max(if self.tagged { 3 } else { 0 }),
                        K::ResourceLimit,
                    )?;
                    if map {
                        let k = self.string()?;
                        self.take(b':')?;
                        m.push((k, self.value(depth + 1)?));
                    } else {
                        a.push(self.value(depth + 1)?);
                    }
                    self.ws();
                    if self.input.get(self.pos) == Some(&close) {
                        self.pos += 1;
                        break;
                    }
                    self.take(b',')?;
                }
                Ok(if map { Json::Map(m) } else { Json::Array(a) })
            }
            b'-' | b'0'..=b'9' => {
                let start = self.pos;
                if self.input[self.pos] == b'-' {
                    self.pos += 1;
                }
                if self.input.get(self.pos) == Some(&b'0') {
                    self.pos += 1;
                } else {
                    ensure(
                        self.input
                            .get(self.pos)
                            .is_some_and(|b| matches!(b, b'1'..=b'9')),
                        K::Syntax,
                    )?;
                    self.digits();
                }
                if self.input.get(self.pos) == Some(&b'.') {
                    self.pos += 1;
                    let p = self.pos;
                    self.digits();
                    ensure(self.pos > p, K::Syntax)?;
                }
                if self
                    .input
                    .get(self.pos)
                    .is_some_and(|b| matches!(b, b'e' | b'E'))
                {
                    self.pos += 1;
                    if self
                        .input
                        .get(self.pos)
                        .is_some_and(|b| matches!(b, b'+' | b'-'))
                    {
                        self.pos += 1;
                    }
                    let p = self.pos;
                    self.digits();
                    ensure(self.pos > p, K::Syntax)?;
                }
                Ok(Json::Number(
                    std::str::from_utf8(&self.input[start..self.pos]).unwrap(),
                ))
            }
            _ => Err(Failure(K::Syntax)),
        }
    }
    fn digits(&mut self) {
        while self.input.get(self.pos).is_some_and(u8::is_ascii_digit) {
            self.pos += 1;
        }
    }
    fn literal(&mut self, s: &[u8]) -> Result<()> {
        ensure(
            self.input.get(self.pos..self.pos + s.len()) == Some(s),
            K::Syntax,
        )?;
        self.pos += s.len();
        Ok(())
    }
}
fn integer(s: &str) -> Result<Value> {
    if s.starts_with('-') {
        s.parse::<i64>()
            .map(Value::Integer)
            .map_err(|_| Failure(K::OutOfRange))
    } else {
        s.parse::<u64>()
            .map(Value::from)
            .map_err(|_| Failure(K::OutOfRange))
    }
}
fn plain(j: Json<'_>, b: &mut Budget<'_>, depth: usize) -> Result<Value> {
    b.node(depth)?;
    Ok(match j {
        Json::Null => Value::Null,
        Json::Bool(x) => Value::Bool(x),
        Json::Number(s) => {
            if s.contains(['.', 'e', 'E']) {
                let f = s.parse::<f64>().map_err(|_| Failure(K::OutOfRange))?;
                ensure(f.is_finite(), K::OutOfRange)?;
                Value::from(f)
            } else {
                integer(s)?
            }
        }
        Json::String(s) => {
            b.payload(s.len(), false)?;
            Value::String(s)
        }
        Json::Array(a) => {
            b.entries(a.len())?;
            Value::Array(
                a.into_iter()
                    .map(|j| plain(j, b, depth + 1))
                    .collect::<Result<_>>()?,
            )
        }
        Json::Map(m) => {
            b.entries(m.len())?;
            let mut out = BTreeMap::new();
            for (k, v) in m {
                b.payload(k.len(), false)?;
                b.key_work(k.len(), out.len() + 1)?;
                ensure(!out.contains_key(&k), K::DuplicateKey)?;
                out.insert(k, plain(v, b, depth + 1)?);
            }
            Value::Map(out)
        }
    })
}
fn array(j: Json<'_>) -> Result<Vec<Json<'_>>> {
    if let Json::Array(a) = j {
        Ok(a)
    } else {
        Err(Failure(K::InvalidNode))
    }
}
fn string(j: Json<'_>) -> Result<String> {
    if let Json::String(s) = j {
        Ok(s)
    } else {
        Err(Failure(K::InvalidNode))
    }
}
fn tagged(j: Json<'_>, b: &mut Budget<'_>, depth: usize) -> Result<Value> {
    b.node(depth)?;
    let a = array(j)?;
    let mut iter = a.into_iter();
    let tag = string(iter.next().ok_or(Failure(K::InvalidNode))?)?;
    if tag == "null" {
        ensure(iter.next().is_none(), K::InvalidNode)?;
        return Ok(Value::Null);
    }
    let x = iter.next().ok_or(Failure(K::InvalidNode))?;
    ensure(iter.next().is_none(), K::InvalidNode)?;
    Ok(match tag.as_str() {
        "bool" => {
            if let Json::Bool(v) = x {
                Value::Bool(v)
            } else {
                return Err(Failure(K::InvalidNode));
            }
        }
        "int" => {
            let s = string(x)?;
            let digits = s.strip_prefix('-').unwrap_or(&s);
            ensure(
                s == "0"
                    || (!digits.is_empty()
                        && digits.as_bytes()[0] != b'0'
                        && digits.bytes().all(|b| b.is_ascii_digit())),
                K::InvalidNode,
            )?;
            integer(&s)?
        }
        "float" => {
            let s = string(x)?;
            ensure(
                s.len() == 16
                    && s.bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
                K::InvalidNode,
            )?;
            Value::from(f64::from_bits(
                u64::from_str_radix(&s, 16).map_err(|_| Failure(K::InvalidNode))?,
            ))
        }
        "string" => {
            let s = string(x)?;
            b.payload(s.len(), false)?;
            Value::String(s)
        }
        "bytes" => {
            let s = string(x)?;
            let n = s.len() / 4 * 3
                - s.bytes()
                    .rev()
                    .take_while(|b| *b == b'=')
                    .count()
                    .min(s.len() / 4 * 3);
            b.payload(n, true)?;
            let bytes = STANDARD.decode(s).map_err(|_| Failure(K::InvalidBase64))?;
            Value::Bytes(bytes)
        }
        "array" => {
            let a = array(x)?;
            b.entries(a.len())?;
            Value::Array(
                a.into_iter()
                    .map(|j| tagged(j, b, depth + 1))
                    .collect::<Result<_>>()?,
            )
        }
        "map" => {
            let a = array(x)?;
            b.entries(a.len())?;
            let mut m = BTreeMap::new();
            for p in a {
                let pair = array(p)?;
                ensure(pair.len() == 2, K::InvalidNode)?;
                let mut pair = pair.into_iter();
                let k = string(pair.next().unwrap())?;
                b.payload(k.len(), false)?;
                b.key_work(k.len(), m.len() + 1)?;
                ensure(!m.contains_key(&k), K::DuplicateKey)?;
                m.insert(k, tagged(pair.next().unwrap(), b, depth + 1)?);
            }
            Value::Map(m)
        }
        _ => return Err(Failure(K::InvalidNode)),
    })
}
pub(crate) fn decode(input: &[u8], limits: &Limits, is_tagged: bool) -> Result<Value> {
    ensure(input.len() <= limits.max_input_bytes, K::ResourceLimit)?;
    std::str::from_utf8(input).map_err(|_| Failure(K::InvalidUnicode))?;
    let mut p = Parser {
        input,
        pos: 0,
        budget: Budget::new(limits),
        tagged: is_tagged,
    };
    p.budget.work(input.len())?;
    let j = p.value(0)?;
    p.ws();
    ensure(p.pos == input.len(), K::Syntax)?;
    if !is_tagged {
        return plain(j, &mut p.budget, 0);
    }
    let envelope = array(j)?;
    ensure(envelope.len() == 3, K::InvalidNode)?;
    let mut e = envelope.into_iter();
    ensure(
        string(e.next().unwrap())? == "structfs-value",
        K::InvalidNode,
    )?;
    let Json::Number(version) = e.next().unwrap() else {
        return Err(Failure(K::InvalidNode));
    };
    ensure(!version.contains(['.', 'e', 'E']), K::InvalidNode)?;
    ensure(version == "1", K::UnsupportedVersion)?;
    tagged(e.next().unwrap(), &mut p.budget, 0)
}

pub(crate) struct Output<'a> {
    pub bytes: Vec<u8>,
    pub limits: &'a Limits,
    pub budget: Budget<'a>,
}
impl<'a> Output<'a> {
    pub fn new(limits: &'a Limits) -> Self {
        Self {
            bytes: Vec::new(),
            limits,
            budget: Budget::new(limits),
        }
    }
    pub fn put(&mut self, s: &[u8]) -> Result<()> {
        let n = self
            .bytes
            .len()
            .checked_add(s.len())
            .ok_or(Failure(K::ResourceLimit))?;
        ensure(
            n <= self.limits.max_output_bytes
                && n <= self.limits.max_allocation_bytes
                && n <= self.limits.max_work,
            K::ResourceLimit,
        )?;
        self.budget.work(s.len())?;
        self.budget.allocate(s.len().saturating_mul(2))?;
        self.bytes.extend_from_slice(s);
        Ok(())
    }
    pub fn quote(&mut self, s: &str) -> Result<()> {
        self.put(b"\"")?;
        for c in s.chars() {
            match c {
                '"' => self.put(b"\\\"")?,
                '\\' => self.put(b"\\\\")?,
                c if (c as u32) < 32 => self.put(format!("\\u{:04x}", c as u32).as_bytes())?,
                c => {
                    self.put(c.encode_utf8(&mut [0; 4]).as_bytes())?;
                }
            }
        }
        self.put(b"\"")
    }
    fn value(&mut self, v: &Value, tagged: bool) -> Result<()> {
        if tagged {
            self.put(b"[")?;
            self.quote(match v {
                Value::Null => "null",
                Value::Bool(_) => "bool",
                Value::Integer(_) | Value::Unsigned(_) => "int",
                Value::Float(_) => "float",
                Value::String(_) => "string",
                Value::Bytes(_) => "bytes",
                Value::Array(_) => "array",
                Value::Map(_) => "map",
                _ => return Err(Failure(K::UnsupportedValue)),
            })?;
            if !v.is_null() {
                self.put(b",")?;
            }
        }
        match v {
            Value::Null => {
                if !tagged {
                    self.put(b"null")?
                }
            }
            Value::Bool(x) => self.put(if *x { b"true" } else { b"false" })?,
            Value::Integer(x) => {
                let s = x.to_string();
                if tagged {
                    self.quote(&s)?
                } else {
                    self.put(s.as_bytes())?
                }
            }
            Value::Unsigned(x) => {
                let s = x.to_string();
                if tagged {
                    self.quote(&s)?
                } else {
                    self.put(s.as_bytes())?
                }
            }
            Value::Float(x) => {
                if tagged {
                    self.quote(&format!(
                        "{:016x}",
                        if x.is_nan() {
                            0x7ff8000000000000
                        } else {
                            x.to_bits()
                        }
                    ))?
                } else {
                    ensure(x.is_finite(), K::UnsupportedValue)?;
                    self.put(
                        serde_json::to_string(x)
                            .map_err(|_| Failure(K::UnsupportedValue))?
                            .as_bytes(),
                    )?
                }
            }
            Value::String(s) => self.quote(s)?,
            Value::Bytes(b) => {
                ensure(tagged, K::UnsupportedValue)?;
                let n = b.len().div_ceil(3).saturating_mul(4);
                ensure(
                    n <= self.limits.max_output_bytes && n <= self.limits.max_allocation_bytes,
                    K::ResourceLimit,
                )?;
                self.quote(&STANDARD.encode(b))?;
            }
            Value::Array(a) => {
                self.put(b"[")?;
                for (i, v) in a.iter().enumerate() {
                    if i != 0 {
                        self.put(b",")?;
                    }
                    self.value(v, tagged)?;
                }
                self.put(b"]")?;
            }
            Value::Map(m) => {
                self.put(if tagged { b"[" } else { b"{" })?;
                for (i, (k, v)) in m.iter().enumerate() {
                    if i != 0 {
                        self.put(b",")?;
                    }
                    if tagged {
                        self.put(b"[")?;
                    }
                    self.quote(k)?;
                    self.put(if tagged { b"," } else { b":" })?;
                    self.value(v, tagged)?;
                    if tagged {
                        self.put(b"]")?;
                    }
                }
                self.put(if tagged { b"]" } else { b"}" })?;
            }
            _ => return Err(Failure(K::UnsupportedValue)),
        }
        if tagged {
            self.put(b"]")?;
        }
        Ok(())
    }
}
pub(crate) fn encode(v: &Value, limits: &Limits, tagged: bool) -> Result<Vec<u8>> {
    let mut out = Output::new(limits);
    out.budget.tree(v, 0)?;
    if tagged {
        out.put(b"[\"structfs-value\",1,")?;
    }
    out.value(v, tagged)?;
    if tagged {
        out.put(b"]")?;
    }
    Ok(out.bytes)
}
