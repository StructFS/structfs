use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use structfs_core_store::CodecErrorKind as K;
use structfs_serde_store::*;
fn kind(e: Error) -> K {
    match e {
        Error::Codec { kind, .. } => kind,
        Error::UnsupportedFormat(_) => K::UnsupportedProfile,
        _ => panic!("unexpected error: {e}"),
    }
}
fn named(k: K) -> String {
    let s = format!("{k:?}");
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if c.is_uppercase() && i > 0 {
            out.push('_');
        }
        out.push(c.to_ascii_lowercase());
    }
    out
}
fn hex(s: &str) -> Vec<u8> {
    s.as_bytes()
        .chunks_exact(2)
        .map(|p| u8::from_str_radix(std::str::from_utf8(p).unwrap(), 16).unwrap())
        .collect()
}
fn corpus() -> serde_json::Value {
    serde_json::from_str(include_str!("fixtures/structfs-value-v1.json")).unwrap()
}
#[test]
fn normative_vectors() {
    let fixture = corpus();
    for (group, profile) in [
        ("tagged_json", Profile::ValueJson),
        ("plain_json", Profile::Json),
        ("cbor", Profile::Cbor),
    ] {
        let c = ValueCodec::new(profile);
        let format = profile.format();
        for v in fixture[group]["accepted"].as_array().unwrap() {
            let bytes = if group == "cbor" {
                hex(v["input_hex"].as_str().unwrap())
            } else {
                v["input"].as_str().unwrap().as_bytes().to_vec()
            };
            let value = c
                .decode(&bytes.clone().into(), &format)
                .unwrap_or_else(|e| panic!("{}: {e}", v["name"]));
            let canonical = ValueJsonCodec.encode(&value, &Format::VALUE_JSON).unwrap();
            let expected = v[if group == "tagged_json" {
                "canonical"
            } else {
                "canonical_value"
            }]
            .as_str()
            .unwrap();
            assert_eq!(canonical.as_ref(), expected.as_bytes(), "{}", v["name"]);
            let encoded = c.encode(&value, &format).unwrap();
            assert!(c.decode(&encoded, &format).unwrap().semantic_eq(&value));
            if group == "cbor" {
                assert_eq!(
                    encoded.as_ref(),
                    hex(v["writer_hex"].as_str().unwrap()),
                    "{}",
                    v["name"]
                );
            }
            if group == "tagged_json" {
                let result = c.clone().canonical().decode(&bytes.clone().into(), &format);
                if bytes == canonical {
                    assert!(result.is_ok());
                } else {
                    assert_eq!(kind(result.unwrap_err()), K::Noncanonical);
                }
            }
        }
        for v in fixture[group]["rejected"].as_array().unwrap() {
            let bytes = if group == "cbor" {
                hex(v["input_hex"].as_str().unwrap())
            } else {
                v["input"].as_str().unwrap().as_bytes().to_vec()
            };
            let result = c.decode(&bytes.into(), &format);
            assert_eq!(
                named(kind(result.unwrap_err())),
                v["error"].as_str().unwrap(),
                "{}",
                v["name"]
            );
        }
    }
    for v in fixture["tagged_json"]["rejected_octets"]
        .as_array()
        .unwrap()
    {
        assert_eq!(
            named(kind(
                ValueJsonCodec
                    .decode(
                        &hex(v["input_hex"].as_str().unwrap()).into(),
                        &Format::VALUE_JSON
                    )
                    .unwrap_err()
            )),
            v["error"].as_str().unwrap()
        );
    }
    let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/specs/fixtures/structfs-value-v1.json");
    if source.exists() {
        assert_eq!(
            std::fs::read_to_string(source).unwrap(),
            include_str!("fixtures/structfs-value-v1.json")
        );
    }
}
#[derive(Debug, Serialize, Deserialize, PartialEq)]
enum View {
    Empty,
    Text(String),
    Pair(u64, bool),
    Record {
        revision: u64,
        state: ExplicitOption<()>,
        payload: Value,
    },
}
#[test]
fn serde_types_and_application_shapes() {
    for view in [
        View::Empty,
        View::Text("hello".into()),
        View::Pair(u64::MAX, true),
        View::Record {
            revision: u64::MAX,
            state: ExplicitOption(Some(())),
            payload: Value::Bytes(vec![0, 255]),
        },
    ] {
        let value = to_value(&view).unwrap();
        for profile in [Profile::ValueJson, Profile::Cbor, Profile::Flexbuffers] {
            let codec = ValueCodec::new(profile);
            let bytes = codec.encode(&value, &profile.format()).unwrap();
            let decoded = codec.decode(&bytes, &profile.format()).unwrap();
            assert_eq!(from_value::<View>(decoded).unwrap(), view);
        }
    }
    assert_eq!(kind(to_value(&Some(())).unwrap_err()), K::AmbiguousOption);
    assert_eq!(
        kind(to_value(&Some(None::<u64>)).unwrap_err()),
        K::AmbiguousOption
    );
    assert_eq!(from_value::<Option<u64>>(Value::Null).unwrap(), None);
    let nested = ExplicitOption(Some(ExplicitOption::<()>(None)));
    assert_eq!(
        from_value::<ExplicitOption<ExplicitOption<()>>>(to_value(&nested).unwrap()).unwrap(),
        nested
    );
    assert!(from_value::<ExplicitOption<()>>(json_to_value(
        serde_json::json!({"kind":"none","value":null})
    ))
    .is_err());
    assert_eq!(
        to_value(&vec![0u8, 255]).unwrap(),
        Value::Array(vec![Value::Integer(0), Value::Integer(255)])
    );
    assert_eq!(
        kind(to_value(&BTreeMap::from([(1u32, true)])).unwrap_err()),
        K::UnsupportedValue
    );
    assert_eq!(
        kind(from_value::<f64>(Value::Integer(1)).unwrap_err()),
        K::TypeMismatch
    );
    assert_eq!(
        kind(from_value::<u8>(Value::Integer(256)).unwrap_err()),
        K::OutOfRange
    );
    assert_eq!(
        kind(from_value::<u64>(Value::Integer(-1)).unwrap_err()),
        K::OutOfRange
    );
    assert_eq!(
        kind(from_value::<f32>(Value::Float(0.1)).unwrap_err()),
        K::OutOfRange
    );
    assert_eq!(
        from_value::<f32>(Value::Float(-0.0)).unwrap().to_bits(),
        (-0.0f32).to_bits()
    );
    assert_eq!(kind(to_value(&u128::MAX).unwrap_err()), K::OutOfRange);
    assert_eq!(kind(to_value(&i128::MIN).unwrap_err()), K::OutOfRange);
    assert!(
        from_value::<View>(Value::Map(BTreeMap::from([("Empty".into(), Value::Null)]))).is_err()
    );
}
fn random(seed: &mut u64, depth: usize) -> Value {
    *seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
    let n = *seed;
    match if depth == 0 { n % 7 } else { n % 9 } {
        0 => Value::Null,
        1 => Value::Bool(n & 128 != 0),
        2 => Value::Integer(n as i64),
        3 => Value::from(n),
        4 => Value::from(f64::from_bits(n)),
        5 => Value::String(format!("é/\0{n}")),
        6 => Value::Bytes(n.to_le_bytes().to_vec()),
        7 => Value::Array((0..n % 5).map(|_| random(seed, depth - 1)).collect()),
        _ => Value::Map(
            (0..n % 5)
                .map(|i| (format!("k{i}"), random(seed, depth - 1)))
                .collect(),
        ),
    }
}
#[test]
fn generated_cross_codec_roundtrips() {
    let mut seed = 42;
    for _ in 0..1000 {
        let v = random(&mut seed, 4);
        assert!(to_value(&v).unwrap().semantic_eq(&v));
        assert!(from_value::<Value>(v.clone()).unwrap().semantic_eq(&v));
        for profile in [Profile::ValueJson, Profile::Cbor, Profile::Flexbuffers] {
            let c = ValueCodec::new(profile);
            let b = c.encode(&v, &profile.format()).unwrap();
            let out = c
                .decode(&b, &profile.format())
                .unwrap_or_else(|e| panic!("{profile:?}: {v:?}, bytes={b:?}: {e}"));
            assert!(v.semantic_eq(&out), "{profile:?}: {v:?} != {out:?}");
        }
        if let Ok(bytes) = JsonCodec.encode(&v, &Format::JSON) {
            assert!(v.semantic_eq(&JsonCodec.decode(&bytes, &Format::JSON).unwrap()));
        }
    }
}
#[test]
fn native_profile_rejections() {
    for v in [
        Value::Bytes(vec![]),
        Value::Float(f64::INFINITY),
        Value::Float(f64::NAN),
    ] {
        assert_eq!(
            kind(JsonCodec.encode(&v, &Format::JSON).unwrap_err()),
            K::UnsupportedValue
        );
    }
    let value = Value::Map(BTreeMap::from([("a\0b".into(), Value::Null)]));
    assert_eq!(
        kind(
            FlexbuffersCodec
                .encode(&value, &Format::FLEXBUFFERS)
                .unwrap_err()
        ),
        K::UnsupportedValue
    );
    assert!(ValueJsonCodec
        .decode(
            &ValueJsonCodec.encode(&value, &Format::VALUE_JSON).unwrap(),
            &Format::VALUE_JSON
        )
        .unwrap()
        .semantic_eq(&value));
    assert!(!Value::Float(0.0).semantic_eq(&Value::Float(-0.0)));
    assert!(Value::Float(f64::NAN).semantic_eq(&Value::Float(f64::from_bits(0xfff0000000000001))));
    assert!(Value::Integer(1).semantic_eq(&Value::Unsigned(1)));
    assert!(!Value::Integer(1).semantic_eq(&Value::Float(1.0)));
}
#[test]
fn limits_are_inclusive_and_apply_to_both_directions() {
    let v = Value::Array(vec![Value::String("abc".into())]);
    for profile in [
        Profile::ValueJson,
        Profile::Json,
        Profile::Cbor,
        Profile::Flexbuffers,
    ] {
        let base = ValueCodec::new(profile);
        let f = profile.format();
        let bytes = base.encode(&v, &f).unwrap();
        let limits = Limits {
            max_depth: 1,
            max_nodes: 2,
            max_collection_entries: 1,
            max_string_bytes: 3,
            max_payload_bytes: 3,
            max_input_bytes: bytes.len(),
            max_output_bytes: bytes.len(),
            ..Limits::default()
        };
        let c = base.with_limits(limits.clone());
        assert!(c.encode(&v, &f).is_ok());
        assert!(c.decode(&bytes, &f).is_ok(), "{profile:?}");
        for change in 0..7 {
            let mut l = limits.clone();
            match change {
                0 => l.max_depth = 0,
                1 => l.max_nodes = 1,
                2 => l.max_collection_entries = 0,
                3 => l.max_string_bytes = 2,
                4 => l.max_payload_bytes = 2,
                5 => {
                    l.max_input_bytes = bytes.len() - 1;
                    l.max_output_bytes = bytes.len() - 1;
                }
                _ => l.max_allocation_bytes = 0,
            };
            let c = ValueCodec::new(profile).with_limits(l);
            assert_eq!(kind(c.encode(&v, &f).unwrap_err()), K::ResourceLimit);
            assert_eq!(kind(c.decode(&bytes, &f).unwrap_err()), K::ResourceLimit);
        }
    }
    assert_eq!(
        kind(
            to_value_with_limits(
                &vec![1, 2],
                &Limits {
                    max_nodes: 2,
                    ..Limits::default()
                }
            )
            .unwrap_err()
        ),
        K::ResourceLimit
    );
}
#[test]
fn malformed_binary_inputs_are_bounded() {
    let limits = Limits {
        max_depth: 8,
        max_nodes: 64,
        max_input_bytes: 1024,
        max_work: 4096,
        ..Limits::default()
    };
    for profile in [Profile::Cbor, Profile::Flexbuffers] {
        let c = ValueCodec::new(profile).with_limits(limits.clone());
        let f = profile.format();
        let original = c
            .encode(
                &Value::Map(BTreeMap::from([(
                    "a".into(),
                    Value::Array(vec![Value::Bool(true), Value::Bytes(vec![1, 2])]),
                )])),
                &f,
            )
            .unwrap();
        for n in 0..original.len() {
            let _ = c.decode(&original.slice(..n), &f);
        }
        for i in 0..original.len() {
            for b in [0, 1, 15, 31, 127, 255] {
                let mut mutated = original.to_vec();
                mutated[i] = b;
                let _ = c.decode(&mutated.into(), &f);
            }
        }
        for a in 0..256u16 {
            let _ = c.decode(&vec![a as u8, 255, 255].into(), &f);
        }
    }
}

#[test]
fn serde_support_matrix() {
    macro_rules! roundtrip {
        ($v:expr,$ty:ty) => {{
            let value: $ty = $v;
            assert_eq!(from_value::<$ty>(to_value(&value).unwrap()).unwrap(), value);
        }};
    }
    roundtrip!(i8::MIN, i8);
    roundtrip!(i16::MIN, i16);
    roundtrip!(i32::MIN, i32);
    roundtrip!(i64::MIN, i64);
    roundtrip!(u64::MAX as i128, i128);
    roundtrip!(u8::MAX, u8);
    roundtrip!(u16::MAX, u16);
    roundtrip!(u32::MAX, u32);
    roundtrip!(u64::MAX, u64);
    roundtrip!(u64::MAX as u128, u128);
    roundtrip!('𐀀', char);
    roundtrip!(1.25, f32);
    roundtrip!(1.25, f64);
    roundtrip!((), ());
    roundtrip!((1u64, true), (u64, bool));
    #[derive(Debug, Serialize, Deserialize, PartialEq)]
    struct Unit;
    #[derive(Debug, Serialize, Deserialize, PartialEq)]
    struct Newtype(String);
    #[derive(Debug, Serialize, Deserialize, PartialEq)]
    struct Tuple(i32, bool);
    #[derive(Debug, Serialize, Deserialize, PartialEq)]
    struct Fields {
        #[serde(flatten)]
        extra: BTreeMap<String, u64>,
    }
    #[derive(Debug, Serialize, Deserialize, PartialEq)]
    #[serde(tag = "kind")]
    enum Internal {
        Item { n: u64 },
    }
    #[derive(Debug, Serialize, Deserialize, PartialEq)]
    #[serde(tag = "kind", content = "value")]
    enum Adjacent {
        Item(u64),
    }
    #[derive(Debug, Serialize, Deserialize, PartialEq)]
    #[serde(untagged)]
    enum Untagged {
        Text(String),
        Flag(bool),
    }
    roundtrip!(Unit, Unit);
    roundtrip!(Newtype("s".into()), Newtype);
    roundtrip!(Tuple(-1, true), Tuple);
    roundtrip!(
        Fields {
            extra: BTreeMap::from([("revision".into(), u64::MAX)])
        },
        Fields
    );
    roundtrip!(Internal::Item { n: u64::MAX }, Internal);
    roundtrip!(Adjacent::Item(u64::MAX), Adjacent);
    roundtrip!(Untagged::Flag(true), Untagged);
    #[derive(Debug, PartialEq)]
    struct Blob(Vec<u8>);
    impl Serialize for Blob {
        fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
            s.serialize_bytes(&self.0)
        }
    }
    impl<'de> Deserialize<'de> for Blob {
        fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
            struct Visitor;
            impl<'de> serde::de::Visitor<'de> for Visitor {
                type Value = Blob;
                fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                    f.write_str("bytes")
                }
                fn visit_bytes<E: serde::de::Error>(self, b: &[u8]) -> Result<Blob, E> {
                    Ok(Blob(b.into()))
                }
            }
            d.deserialize_byte_buf(Visitor)
        }
    }
    roundtrip!(Blob(vec![0, 255]), Blob);
    struct Human;
    impl Serialize for Human {
        fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
            assert!(s.is_human_readable());
            s.collect_str(&"displayed")
        }
    }
    assert_eq!(to_value(&Human).unwrap(), Value::String("displayed".into()));
    assert_eq!(
        kind(
            to_value_with_limits(
                &Human,
                &Limits {
                    max_string_bytes: 1,
                    ..Limits::default()
                }
            )
            .unwrap_err()
        ),
        K::ResourceLimit
    );
    assert!(from_value::<(u64, bool)>(Value::Array(vec![Value::Integer(1)])).is_err());
    assert!(from_value::<char>(Value::String("ab".into())).is_err());
    assert!(from_value::<Vec<u8>>(Value::Bytes(vec![1])).is_err());
}

#[test]
fn malformed_custom_serializers_return_errors() {
    struct Broken(u8);
    impl Serialize for Broken {
        fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
            use serde::ser::{SerializeMap, SerializeSeq};
            if self.0 == 0 {
                let mut m = s.serialize_map(None)?;
                m.serialize_entry("a", &1)?;
                m.serialize_entry("a", &2)?;
                m.end()
            } else if self.0 == 1 {
                let mut m = s.serialize_map(None)?;
                m.serialize_value(&1)?;
                m.end()
            } else if self.0 == 2 {
                let mut m = s.serialize_map(None)?;
                m.serialize_key("a")?;
                m.end()
            } else {
                let mut a = s.serialize_seq(Some(2))?;
                a.serialize_element(&1)?;
                a.end()
            }
        }
    }
    assert_eq!(kind(to_value(&Broken(0)).unwrap_err()), K::DuplicateKey);
    for i in 1..4 {
        assert_eq!(kind(to_value(&Broken(i)).unwrap_err()), K::TypeMismatch);
    }
}

#[test]
fn transcode_validates_and_rejects_subset_loss() {
    let tagged = ValueCodec::new(Profile::ValueJson);
    let pretty = Bytes::from_static(b" [\"structfs-value\", 1, [\"null\"]] ");
    assert_eq!(
        transcode(&pretty, &tagged, &tagged).unwrap().as_ref(),
        b"[\"structfs-value\",1,[\"null\"]]"
    );
    assert!(transcode(&Bytes::from_static(b"invalid"), &tagged, &tagged).is_err());
    let encoded = tagged
        .encode(&Value::Bytes(vec![0]), &Format::VALUE_JSON)
        .unwrap();
    assert_eq!(
        kind(transcode(&encoded, &tagged, &ValueCodec::new(Profile::Json)).unwrap_err()),
        K::UnsupportedValue
    );
    let c = tagged.with_limits(Limits {
        max_diagnostic_bytes: 0,
        ..Limits::default()
    });
    let Error::Codec { message, .. } = c.decode(&Bytes::new(), &Format::VALUE_JSON).unwrap_err()
    else {
        panic!()
    };
    assert!(message.is_empty());
}

#[test]
fn deterministic_fuzz_decoding_and_canonicalization() {
    let limits = Limits {
        max_depth: 8,
        max_nodes: 128,
        max_input_bytes: 512,
        max_output_bytes: 4096,
        max_work: 8192,
        ..Limits::default()
    };
    let mut seed = 17u64;
    for _ in 0..4096 {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        let mut bytes = Vec::new();
        for _ in 0..seed as usize % 96 {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            bytes.push((seed >> 32) as u8);
        }
        for profile in [
            Profile::ValueJson,
            Profile::Json,
            Profile::Cbor,
            Profile::Flexbuffers,
        ] {
            let c = ValueCodec::new(profile).with_limits(limits.clone());
            if let Ok(value) = c.decode(&bytes.clone().into(), &profile.format()) {
                let canonical = ValueJsonCodec.encode(&value, &Format::VALUE_JSON).unwrap();
                assert!(value.semantic_eq(
                    &ValueCodec::new(Profile::ValueJson)
                        .canonical()
                        .decode(&canonical, &Format::VALUE_JSON)
                        .unwrap()
                ));
            }
        }
    }
}
