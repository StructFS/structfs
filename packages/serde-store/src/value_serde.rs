//! Direct, checked `structfs-serde/1` conversion.
use crate::limits::{ensure, Budget, Failure, Limits, Result};
use serde::{de, ser, Deserialize, Serialize};
use std::collections::BTreeMap;
use structfs_core_store::{CodecErrorKind as K, CodecOperation, Error, Format, Value};

pub fn to_value_with_limits<T: Serialize + ?Sized>(
    value: &T,
    limits: &Limits,
) -> std::result::Result<Value, Error> {
    value
        .serialize(Serializer {
            budget: &mut Budget::new(limits),
            depth: 0,
            key: false,
        })
        .map_err(|e| e.core(&Format::VALUE, CodecOperation::Encode, limits))
}
pub fn from_value_with_limits<T: de::DeserializeOwned>(
    value: Value,
    limits: &Limits,
) -> std::result::Result<T, Error> {
    Budget::new(limits)
        .tree(&value, 0)
        .and_then(|()| T::deserialize(Deserializer(&value)))
        .map_err(|e| e.core(&Format::VALUE, CodecOperation::Decode, limits))
}
pub fn to_value<T: Serialize + ?Sized>(value: &T) -> std::result::Result<Value, Error> {
    to_value_with_limits(value, &Limits::default())
}
pub fn from_value<T: de::DeserializeOwned>(value: Value) -> std::result::Result<T, Error> {
    from_value_with_limits(value, &Limits::default())
}

struct Serializer<'a, 'b> {
    budget: &'a mut Budget<'b>,
    depth: usize,
    key: bool,
}
impl<'a, 'b> Serializer<'a, 'b> {
    fn scalar(self, v: Value) -> Result<Value> {
        ensure(!self.key, K::UnsupportedValue)?;
        self.budget.node(self.depth)?;
        Ok(v)
    }
    fn compound(self, len: Option<usize>, map: bool) -> Result<Compound<'a, 'b>> {
        ensure(!self.key, K::UnsupportedValue)?;
        self.budget.node(self.depth)?;
        if let Some(n) = len {
            self.budget.entries(n)?;
        }
        Ok(Compound {
            serializer: self,
            expected: len,
            array: Vec::new(),
            map: BTreeMap::new(),
            key: None,
            is_map: map,
            variant: None,
        })
    }
}
impl<'a, 'b> ser::Serializer for Serializer<'a, 'b> {
    type Ok = Value;
    type Error = Failure;
    type SerializeSeq = Compound<'a, 'b>;
    type SerializeTuple = Compound<'a, 'b>;
    type SerializeTupleStruct = Compound<'a, 'b>;
    type SerializeTupleVariant = Compound<'a, 'b>;
    type SerializeMap = Compound<'a, 'b>;
    type SerializeStruct = Compound<'a, 'b>;
    type SerializeStructVariant = Compound<'a, 'b>;
    fn serialize_bool(self, v: bool) -> Result<Value> {
        self.scalar(Value::Bool(v))
    }
    fn serialize_i8(self, v: i8) -> Result<Value> {
        self.serialize_i64(v.into())
    }
    fn serialize_i16(self, v: i16) -> Result<Value> {
        self.serialize_i64(v.into())
    }
    fn serialize_i32(self, v: i32) -> Result<Value> {
        self.serialize_i64(v.into())
    }
    fn serialize_i64(self, v: i64) -> Result<Value> {
        self.scalar(Value::Integer(v))
    }
    fn serialize_i128(self, v: i128) -> Result<Value> {
        if v < 0 {
            self.serialize_i64(v.try_into().map_err(|_| Failure(K::OutOfRange))?)
        } else {
            self.serialize_u64(v.try_into().map_err(|_| Failure(K::OutOfRange))?)
        }
    }
    fn serialize_u8(self, v: u8) -> Result<Value> {
        self.serialize_u64(v.into())
    }
    fn serialize_u16(self, v: u16) -> Result<Value> {
        self.serialize_u64(v.into())
    }
    fn serialize_u32(self, v: u32) -> Result<Value> {
        self.serialize_u64(v.into())
    }
    fn serialize_u64(self, v: u64) -> Result<Value> {
        self.scalar(Value::from(v))
    }
    fn serialize_u128(self, v: u128) -> Result<Value> {
        self.serialize_u64(v.try_into().map_err(|_| Failure(K::OutOfRange))?)
    }
    fn serialize_f32(self, v: f32) -> Result<Value> {
        self.serialize_f64(v.into())
    }
    fn serialize_f64(self, v: f64) -> Result<Value> {
        self.scalar(Value::from(v))
    }
    fn serialize_char(self, v: char) -> Result<Value> {
        ensure(!self.key, K::UnsupportedValue)?;
        self.serialize_str(v.encode_utf8(&mut [0; 4]))
    }
    fn serialize_str(self, v: &str) -> Result<Value> {
        if !self.key {
            self.budget.node(self.depth)?;
        }
        self.budget.payload(v.len(), false)?;
        Ok(Value::String(v.into()))
    }
    fn serialize_bytes(self, v: &[u8]) -> Result<Value> {
        ensure(!self.key, K::UnsupportedValue)?;
        self.budget.payload(v.len(), true)?;
        self.scalar(Value::Bytes(v.into()))
    }
    fn serialize_none(self) -> Result<Value> {
        self.scalar(Value::Null)
    }
    fn serialize_some<T: Serialize + ?Sized>(self, v: &T) -> Result<Value> {
        ensure(!self.key, K::UnsupportedValue)?;
        let value = v.serialize(self)?;
        ensure(!value.is_null(), K::AmbiguousOption)?;
        Ok(value)
    }
    fn serialize_unit(self) -> Result<Value> {
        self.scalar(Value::Null)
    }
    fn serialize_unit_struct(self, _: &'static str) -> Result<Value> {
        self.serialize_unit()
    }
    fn serialize_unit_variant(self, _: &'static str, _: u32, v: &'static str) -> Result<Value> {
        ensure(!self.key, K::UnsupportedValue)?;
        self.serialize_str(v)
    }
    fn serialize_newtype_struct<T: Serialize + ?Sized>(
        self,
        _: &'static str,
        v: &T,
    ) -> Result<Value> {
        v.serialize(self)
    }
    fn serialize_newtype_variant<T: Serialize + ?Sized>(
        self,
        _: &'static str,
        _: u32,
        k: &'static str,
        v: &T,
    ) -> Result<Value> {
        let mut c = self.compound(Some(1), true)?;
        ser::SerializeMap::serialize_entry(&mut c, k, v)?;
        c.finish()
    }
    fn serialize_seq(self, n: Option<usize>) -> Result<Self::SerializeSeq> {
        self.compound(n, false)
    }
    fn serialize_tuple(self, n: usize) -> Result<Self::SerializeTuple> {
        self.compound(Some(n), false)
    }
    fn serialize_tuple_struct(
        self,
        _: &'static str,
        n: usize,
    ) -> Result<Self::SerializeTupleStruct> {
        self.compound(Some(n), false)
    }
    fn serialize_tuple_variant(
        self,
        _: &'static str,
        _: u32,
        k: &'static str,
        n: usize,
    ) -> Result<Self::SerializeTupleVariant> {
        self.variant(k, n, false)
    }
    fn serialize_map(self, n: Option<usize>) -> Result<Self::SerializeMap> {
        self.compound(n, true)
    }
    fn serialize_struct(self, _: &'static str, n: usize) -> Result<Self::SerializeStruct> {
        self.compound(Some(n), true)
    }
    fn serialize_struct_variant(
        self,
        _: &'static str,
        _: u32,
        k: &'static str,
        n: usize,
    ) -> Result<Self::SerializeStructVariant> {
        self.variant(k, n, true)
    }
    fn is_human_readable(&self) -> bool {
        true
    }
    fn collect_str<T: std::fmt::Display + ?Sized>(self, value: &T) -> Result<Value> {
        use std::fmt::Write;
        ensure(!self.key, K::UnsupportedValue)?;
        struct Bounded {
            text: String,
            max: usize,
        }
        impl std::fmt::Write for Bounded {
            fn write_str(&mut self, s: &str) -> std::fmt::Result {
                if s.len() > self.max.saturating_sub(self.text.len()) {
                    return Err(std::fmt::Error);
                }
                self.text.push_str(s);
                Ok(())
            }
        }
        let max = self
            .budget
            .limits
            .max_string_bytes
            .min(self.budget.limits.max_allocation_bytes)
            .min(self.budget.limits.max_work);
        let mut out = Bounded {
            text: String::new(),
            max,
        };
        write!(&mut out, "{value}").map_err(|_| Failure(K::ResourceLimit))?;
        self.serialize_str(&out.text)
    }
}
impl<'a, 'b> Serializer<'a, 'b> {
    fn variant(mut self, k: &'static str, n: usize, map: bool) -> Result<Compound<'a, 'b>> {
        ensure(!self.key, K::UnsupportedValue)?;
        self.budget.node(self.depth)?;
        self.budget.entries(1)?;
        self.budget.payload(k.len(), false)?;
        self.depth += 1;
        let mut c = self.compound(Some(n), map)?;
        c.variant = Some(k);
        Ok(c)
    }
}
struct Compound<'a, 'b> {
    serializer: Serializer<'a, 'b>,
    expected: Option<usize>,
    array: Vec<Value>,
    map: BTreeMap<String, Value>,
    key: Option<String>,
    is_map: bool,
    variant: Option<&'static str>,
}
impl Compound<'_, '_> {
    fn element<T: Serialize + ?Sized>(&mut self, v: &T) -> Result<()> {
        self.serializer.budget.entries(self.array.len() + 1)?;
        let value = v.serialize(Serializer {
            budget: self.serializer.budget,
            depth: self.serializer.depth + 1,
            key: false,
        })?;
        self.array.push(value);
        Ok(())
    }
    fn finish(self) -> Result<Value> {
        ensure(self.key.is_none(), K::TypeMismatch)?;
        let n = if self.is_map {
            self.map.len()
        } else {
            self.array.len()
        };
        ensure(self.expected.is_none_or(|e| e == n), K::TypeMismatch)?;
        let v = if self.is_map {
            Value::Map(self.map)
        } else {
            Value::Array(self.array)
        };
        Ok(if let Some(k) = self.variant {
            Value::Map(BTreeMap::from([(k.to_owned(), v)]))
        } else {
            v
        })
    }
}
impl ser::SerializeMap for Compound<'_, '_> {
    type Ok = Value;
    type Error = Failure;
    fn serialize_key<T: Serialize + ?Sized>(&mut self, k: &T) -> Result<()> {
        ensure(self.key.is_none(), K::TypeMismatch)?;
        self.serializer.budget.entries(self.map.len() + 1)?;
        let key = k.serialize(Serializer {
            budget: self.serializer.budget,
            depth: self.serializer.depth,
            key: true,
        })?;
        let Value::String(key) = key else {
            return Err(Failure(K::UnsupportedValue));
        };
        self.serializer
            .budget
            .key_work(key.len(), self.map.len() + 1)?;
        ensure(!self.map.contains_key(&key), K::DuplicateKey)?;
        self.key = Some(key);
        Ok(())
    }
    fn serialize_value<T: Serialize + ?Sized>(&mut self, v: &T) -> Result<()> {
        let k = self.key.take().ok_or(Failure(K::TypeMismatch))?;
        let v = v.serialize(Serializer {
            budget: self.serializer.budget,
            depth: self.serializer.depth + 1,
            key: false,
        })?;
        self.map.insert(k, v);
        Ok(())
    }
    fn end(self) -> Result<Value> {
        self.finish()
    }
}
macro_rules! seq_impl {
    ($trait:ident,$method:ident) => {
        impl ser::$trait for Compound<'_, '_> {
            type Ok = Value;
            type Error = Failure;
            fn $method<T: Serialize + ?Sized>(&mut self, v: &T) -> Result<()> {
                self.element(v)
            }
            fn end(self) -> Result<Value> {
                self.finish()
            }
        }
    };
}
seq_impl!(SerializeSeq, serialize_element);
seq_impl!(SerializeTuple, serialize_element);
seq_impl!(SerializeTupleStruct, serialize_field);
seq_impl!(SerializeTupleVariant, serialize_field);
macro_rules! struct_impl {
    ($trait:ident) => {
        impl ser::$trait for Compound<'_, '_> {
            type Ok = Value;
            type Error = Failure;
            fn serialize_field<T: Serialize + ?Sized>(
                &mut self,
                k: &'static str,
                v: &T,
            ) -> Result<()> {
                ser::SerializeMap::serialize_entry(self, k, v)
            }
            fn end(self) -> Result<Value> {
                self.finish()
            }
        }
    };
}
struct_impl!(SerializeStruct);
struct_impl!(SerializeStructVariant);

struct Deserializer<'de>(&'de Value);
macro_rules! integer {
    ($method:ident,$visit:ident,$ty:ty) => {
        fn $method<V: de::Visitor<'de>>(self, v: V) -> Result<V::Value> {
            let n = match self.0 {
                Value::Integer(n) => *n as i128,
                Value::Unsigned(n) => *n as i128,
                _ => return Err(Failure(K::TypeMismatch)),
            };
            v.$visit(<$ty>::try_from(n).map_err(|_| Failure(K::OutOfRange))?)
        }
    };
}
impl<'de> de::Deserializer<'de> for Deserializer<'de> {
    type Error = Failure;
    fn deserialize_any<V: de::Visitor<'de>>(self, v: V) -> Result<V::Value> {
        match self.0 {
            Value::Null => v.visit_unit(),
            Value::Bool(x) => v.visit_bool(*x),
            Value::Integer(x) => v.visit_i64(*x),
            Value::Unsigned(x) if *x <= i64::MAX as u64 => v.visit_i64(*x as i64),
            Value::Unsigned(x) => v.visit_u64(*x),
            Value::Float(x) => v.visit_f64(if x.is_nan() {
                f64::from_bits(0x7ff8000000000000)
            } else {
                *x
            }),
            Value::String(x) => v.visit_borrowed_str(x),
            Value::Bytes(x) => v.visit_borrowed_bytes(x),
            Value::Array(_) => self.deserialize_seq(v),
            Value::Map(_) => self.deserialize_map(v),
            _ => Err(Failure(K::UnsupportedValue)),
        }
    }
    integer!(deserialize_i8, visit_i8, i8);
    integer!(deserialize_i16, visit_i16, i16);
    integer!(deserialize_i32, visit_i32, i32);
    integer!(deserialize_i64, visit_i64, i64);
    integer!(deserialize_i128, visit_i128, i128);
    integer!(deserialize_u8, visit_u8, u8);
    integer!(deserialize_u16, visit_u16, u16);
    integer!(deserialize_u32, visit_u32, u32);
    integer!(deserialize_u64, visit_u64, u64);
    integer!(deserialize_u128, visit_u128, u128);
    fn deserialize_bool<V: de::Visitor<'de>>(self, v: V) -> Result<V::Value> {
        if let Value::Bool(x) = self.0 {
            v.visit_bool(*x)
        } else {
            Err(Failure(K::TypeMismatch))
        }
    }
    fn deserialize_f64<V: de::Visitor<'de>>(self, v: V) -> Result<V::Value> {
        if let Value::Float(_) = self.0 {
            self.deserialize_any(v)
        } else {
            Err(Failure(K::TypeMismatch))
        }
    }
    fn deserialize_f32<V: de::Visitor<'de>>(self, v: V) -> Result<V::Value> {
        let Value::Float(x) = self.0 else {
            return Err(Failure(K::TypeMismatch));
        };
        if x.is_nan() {
            return v.visit_f32(f32::from_bits(0x7fc00000));
        }
        let f = *x as f32;
        ensure((f as f64).to_bits() == x.to_bits(), K::OutOfRange)?;
        v.visit_f32(f)
    }
    fn deserialize_str<V: de::Visitor<'de>>(self, v: V) -> Result<V::Value> {
        if let Value::String(x) = self.0 {
            v.visit_borrowed_str(x)
        } else {
            Err(Failure(K::TypeMismatch))
        }
    }
    fn deserialize_string<V: de::Visitor<'de>>(self, v: V) -> Result<V::Value> {
        self.deserialize_str(v)
    }
    fn deserialize_char<V: de::Visitor<'de>>(self, v: V) -> Result<V::Value> {
        let Value::String(x) = self.0 else {
            return Err(Failure(K::TypeMismatch));
        };
        let mut c = x.chars();
        let first = c.next().ok_or(Failure(K::TypeMismatch))?;
        ensure(c.next().is_none(), K::TypeMismatch)?;
        v.visit_char(first)
    }
    fn deserialize_bytes<V: de::Visitor<'de>>(self, v: V) -> Result<V::Value> {
        if let Value::Bytes(x) = self.0 {
            v.visit_borrowed_bytes(x)
        } else {
            Err(Failure(K::TypeMismatch))
        }
    }
    fn deserialize_byte_buf<V: de::Visitor<'de>>(self, v: V) -> Result<V::Value> {
        self.deserialize_bytes(v)
    }
    fn deserialize_option<V: de::Visitor<'de>>(self, v: V) -> Result<V::Value> {
        if self.0.is_null() {
            v.visit_none()
        } else {
            v.visit_some(self)
        }
    }
    fn deserialize_unit<V: de::Visitor<'de>>(self, v: V) -> Result<V::Value> {
        ensure(self.0.is_null(), K::TypeMismatch)?;
        v.visit_unit()
    }
    fn deserialize_unit_struct<V: de::Visitor<'de>>(
        self,
        _: &'static str,
        v: V,
    ) -> Result<V::Value> {
        self.deserialize_unit(v)
    }
    fn deserialize_newtype_struct<V: de::Visitor<'de>>(
        self,
        _: &'static str,
        v: V,
    ) -> Result<V::Value> {
        v.visit_newtype_struct(self)
    }
    fn deserialize_seq<V: de::Visitor<'de>>(self, v: V) -> Result<V::Value> {
        let Value::Array(a) = self.0 else {
            return Err(Failure(K::TypeMismatch));
        };
        let mut access = Sequence(a.iter());
        let result = v.visit_seq(&mut access)?;
        ensure(access.0.len() == 0, K::TypeMismatch)?;
        Ok(result)
    }
    fn deserialize_tuple<V: de::Visitor<'de>>(self, n: usize, v: V) -> Result<V::Value> {
        ensure(
            matches!(self.0,Value::Array(a) if a.len()==n),
            K::TypeMismatch,
        )?;
        self.deserialize_seq(v)
    }
    fn deserialize_tuple_struct<V: de::Visitor<'de>>(
        self,
        _: &'static str,
        n: usize,
        v: V,
    ) -> Result<V::Value> {
        self.deserialize_tuple(n, v)
    }
    fn deserialize_map<V: de::Visitor<'de>>(self, v: V) -> Result<V::Value> {
        let Value::Map(m) = self.0 else {
            return Err(Failure(K::TypeMismatch));
        };
        let mut access = Mapping {
            iter: m.iter(),
            pending: None,
        };
        let result = v.visit_map(&mut access)?;
        ensure(
            access.iter.len() == 0 && access.pending.is_none(),
            K::TypeMismatch,
        )?;
        Ok(result)
    }
    fn deserialize_struct<V: de::Visitor<'de>>(
        self,
        _: &'static str,
        _: &'static [&'static str],
        v: V,
    ) -> Result<V::Value> {
        self.deserialize_map(v)
    }
    fn deserialize_enum<V: de::Visitor<'de>>(
        self,
        _: &'static str,
        _: &'static [&'static str],
        v: V,
    ) -> Result<V::Value> {
        let (name, payload) = match self.0 {
            Value::String(s) => (s.as_str(), None),
            Value::Map(m) if m.len() == 1 => {
                let (k, v) = m.first_key_value().unwrap();
                (k.as_str(), Some(v))
            }
            _ => return Err(Failure(K::TypeMismatch)),
        };
        v.visit_enum(Enum { name, payload })
    }
    fn deserialize_identifier<V: de::Visitor<'de>>(self, v: V) -> Result<V::Value> {
        self.deserialize_str(v)
    }
    fn deserialize_ignored_any<V: de::Visitor<'de>>(self, v: V) -> Result<V::Value> {
        v.visit_unit()
    }
    fn is_human_readable(&self) -> bool {
        true
    }
}
struct Sequence<'a>(std::slice::Iter<'a, Value>);
impl<'de> de::SeqAccess<'de> for Sequence<'de> {
    type Error = Failure;
    fn next_element_seed<T: de::DeserializeSeed<'de>>(&mut self, s: T) -> Result<Option<T::Value>> {
        self.0
            .next()
            .map(|v| s.deserialize(Deserializer(v)))
            .transpose()
    }
    fn size_hint(&self) -> Option<usize> {
        Some(self.0.len())
    }
}
struct Mapping<'a> {
    iter: std::collections::btree_map::Iter<'a, String, Value>,
    pending: Option<&'a Value>,
}
impl<'de> de::MapAccess<'de> for Mapping<'de> {
    type Error = Failure;
    fn next_key_seed<T: de::DeserializeSeed<'de>>(&mut self, s: T) -> Result<Option<T::Value>> {
        ensure(self.pending.is_none(), K::TypeMismatch)?;
        if let Some((k, v)) = self.iter.next() {
            self.pending = Some(v);
            s.deserialize(de::value::BorrowedStrDeserializer::<Failure>::new(k))
                .map(Some)
        } else {
            Ok(None)
        }
    }
    fn next_value_seed<T: de::DeserializeSeed<'de>>(&mut self, s: T) -> Result<T::Value> {
        s.deserialize(Deserializer(
            self.pending.take().ok_or(Failure(K::TypeMismatch))?,
        ))
    }
    fn size_hint(&self) -> Option<usize> {
        Some(self.iter.len())
    }
}
struct Enum<'a> {
    name: &'a str,
    payload: Option<&'a Value>,
}
impl<'de> de::EnumAccess<'de> for Enum<'de> {
    type Error = Failure;
    type Variant = Self;
    fn variant_seed<T: de::DeserializeSeed<'de>>(self, s: T) -> Result<(T::Value, Self)> {
        let n = s.deserialize(de::value::BorrowedStrDeserializer::<Failure>::new(
            self.name,
        ))?;
        Ok((n, self))
    }
}
impl<'de> de::VariantAccess<'de> for Enum<'de> {
    type Error = Failure;
    fn unit_variant(self) -> Result<()> {
        ensure(self.payload.is_none(), K::TypeMismatch)
    }
    fn newtype_variant_seed<T: de::DeserializeSeed<'de>>(self, s: T) -> Result<T::Value> {
        s.deserialize(Deserializer(self.payload.ok_or(Failure(K::TypeMismatch))?))
    }
    fn tuple_variant<V: de::Visitor<'de>>(self, n: usize, v: V) -> Result<V::Value> {
        de::Deserializer::deserialize_tuple(
            Deserializer(self.payload.ok_or(Failure(K::TypeMismatch))?),
            n,
            v,
        )
    }
    fn struct_variant<V: de::Visitor<'de>>(
        self,
        _: &'static [&'static str],
        v: V,
    ) -> Result<V::Value> {
        de::Deserializer::deserialize_map(
            Deserializer(self.payload.ok_or(Failure(K::TypeMismatch))?),
            v,
        )
    }
}

/// Schema-visible option that can preserve Some(()) and nested optional states.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExplicitOption<T>(pub Option<T>);
impl<T: Serialize> Serialize for ExplicitOption<T> {
    fn serialize<S: ser::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        use ser::SerializeMap;
        let mut m = s.serialize_map(Some(if self.0.is_some() { 2 } else { 1 }))?;
        m.serialize_entry("kind", if self.0.is_some() { "some" } else { "none" })?;
        if let Some(v) = &self.0 {
            m.serialize_entry("value", v)?;
        }
        m.end()
    }
}
impl<'de, T: Deserialize<'de>> Deserialize<'de> for ExplicitOption<T> {
    fn deserialize<D: de::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        struct Visitor<T>(std::marker::PhantomData<T>);
        impl<'de, T: Deserialize<'de>> de::Visitor<'de> for Visitor<T> {
            type Value = ExplicitOption<T>;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("an explicit option map")
            }
            fn visit_map<A: de::MapAccess<'de>>(
                self,
                mut a: A,
            ) -> std::result::Result<Self::Value, A::Error> {
                let mut kind: Option<String> = None;
                let mut value: Option<T> = None;
                while let Some(k) = a.next_key::<String>()? {
                    match k.as_str() {
                        "kind" if kind.is_none() => kind = Some(a.next_value()?),
                        "value" if value.is_none() => value = Some(a.next_value()?),
                        _ => {
                            return Err(de::Error::custom(
                                "extra or repeated explicit option field",
                            ))
                        }
                    }
                }
                match (kind.as_deref(), value) {
                    (Some("none"), None) => Ok(ExplicitOption(None)),
                    (Some("some"), Some(v)) => Ok(ExplicitOption(Some(v))),
                    _ => Err(de::Error::custom("invalid explicit option shape")),
                }
            }
        }
        d.deserialize_map(Visitor(std::marker::PhantomData))
    }
}
