use serde::{Deserialize, Serialize};
use structfs_core_store::{CodecErrorKind, Error, Value};
use structfs_serde_store::{from_value, from_value_with_limits, json_to_value, to_value, Limits};
#[derive(Debug, Deserialize)]
enum Mode {
    Streaming,
}
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct Config {
    modes: Vec<Mode>,
}
#[test]
fn unknown_variant_keeps_message_category_and_nested_location() {
    let error = from_value::<Config>(json_to_value(serde_json::json!({"modes":["Misspelled"]})))
        .unwrap_err();
    match error {
        Error::Codec { kind, message, .. } => {
            assert_eq!(kind, CodecErrorKind::TypeMismatch);
            for part in ["modes", "[0]", "Misspelled", "Streaming"] {
                assert!(message.contains(part), "{message}");
            }
        }
        _ => panic!("expected codec error"),
    }
}
#[test]
fn unicode_diagnostics_respect_zero_and_small_byte_limits() {
    for cap in [0, 1, 2, 19, 25, 256, 2048] {
        let limits = Limits {
            max_diagnostic_bytes: cap,
            ..Limits::default()
        };
        let error =
            from_value_with_limits::<Mode>(Value::from("é".repeat(10_000)), &limits).unwrap_err();
        if let Error::Codec { message, .. } = error {
            assert!(message.len() <= cap);
            assert!(message.len() <= 1024);
        } else {
            panic!("expected codec error");
        }
    }
}
struct Invalid;
impl Serialize for Invalid {
    fn serialize<S: serde::Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
        Err(serde::ser::Error::custom("application validation failed"))
    }
}
#[test]
fn custom_serialization_error_retains_field() {
    #[derive(Serialize)]
    struct Request {
        field: Invalid,
    }
    let error = to_value(&Request { field: Invalid })
        .unwrap_err()
        .to_string();
    assert!(error.contains("field"));
    assert!(error.contains("application validation failed"));
}
