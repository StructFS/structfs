use structfs_core_store::{Codec, Value};
use structfs_serde_store::{from_value, to_value, Profile, ValueCodec};
use structfs_state::*;
#[test]
fn portable_protocol_preserves_values_and_typed_expiry_across_profiles() {
    let request = Request::new(Command::Batch {
        expected: None,
        mutations: vec![
            Mutation::Set {
                path: "payload".into(),
                value: Value::Bytes(vec![0, 255]),
            },
            Mutation::Set {
                path: "counter".into(),
                value: Value::Unsigned(u64::MAX),
            },
        ],
    });
    let value = to_value(&request).unwrap();
    for profile in [Profile::ValueJson, Profile::Cbor, Profile::Flexbuffers] {
        let codec = ValueCodec::new(profile);
        let data = codec.encode(&value, &profile.format()).unwrap();
        let decoded: Request = from_value(codec.decode(&data, &profile.format()).unwrap()).unwrap();
        assert_eq!(to_value(&decoded).unwrap(), value);
        let reply: Reply<ChangePage> = Reply::Error(Fault::CursorExpired {
            earliest: Token {
                epoch: "test".into(),
                revision: u64::MAX,
            },
        });
        let decoded: Reply<ChangePage> = from_value(
            codec
                .decode(
                    &codec
                        .encode(&to_value(&reply).unwrap(), &profile.format())
                        .unwrap(),
                    &profile.format(),
                )
                .unwrap(),
        )
        .unwrap();
        assert!(
            matches!(decoded,Reply::Error(Fault::CursorExpired{earliest}) if earliest.revision==u64::MAX)
        );
    }
}
