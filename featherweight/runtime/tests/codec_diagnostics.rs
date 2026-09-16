use featherweight_runtime::{
    protocol::{decode_read_response, error_to_response},
    CoreWasmEngine, ExecutionPolicy,
};
use std::sync::Arc;
use structfs_core_store::{
    path, CodecErrorKind, CodecOperation, Error, Format, NoCodec, Path, Reader, Record, Value,
    Writer,
};
use structfs_serde_store::JsonCodec;
use structfs_service::CleanupSupervisor;
struct Host {
    kind: usize,
    captured: Option<Value>,
}
impl Host {
    fn error(&self) -> Error {
        if self.kind == 2 {
            Error::UnsupportedFormat(Format::JSON)
        } else {
            Error::Codec {
                kind: if self.kind == 0 {
                    CodecErrorKind::TypeMismatch
                } else {
                    CodecErrorKind::ResourceLimit
                },
                operation: CodecOperation::Decode,
                format: Format::JSON,
                message: "original diagnostic".into(),
            }
        }
    }
}
impl Reader for Host {
    fn read(&mut self, _: &Path) -> Result<Option<Record>, Error> {
        Err(self.error())
    }
}
impl Writer for Host {
    fn write(&mut self, p: &Path, r: Record) -> Result<Path, Error> {
        if p == &path!("capture") {
            self.captured = Some(r.into_value(&NoCodec)?);
            Ok(p.clone())
        } else {
            Err(self.error())
        }
    }
}
#[async_trait::async_trait]
impl structfs_core_store::AsyncReader for Host {
    async fn read_async(&mut self, p: &Path) -> Result<Option<Record>, Error> {
        tokio::task::yield_now().await;
        self.read(p)
    }
}
#[async_trait::async_trait]
impl structfs_core_store::AsyncWriter for Host {
    async fn write_async(&mut self, p: &Path, r: Record) -> Result<Path, Error> {
        tokio::task::yield_now().await;
        self.write(p, r)
    }
}
#[tokio::test]
async fn codec_details_cross_read_write_imports_and_serving_envelopes() {
    let engine = CoreWasmEngine::new(1).unwrap();
    let supervisor = CleanupSupervisor::new(1).unwrap();
    for write in [false, true] {
        let call = if write {
            "i32.const 100 i32.const 3 i32.const 200 i32.const 4 i32.const 1024 call $write"
        } else {
            "i32.const 100 i32.const 3 i32.const 1024 call $read"
        };
        let wat = format!(
            r#"(module
            (import "structfs" "read" (func $read (param i32 i32 i32) (result i32)))
            (import "structfs" "write" (func $write (param i32 i32 i32 i32 i32) (result i32)))
            (memory (export "memory") 1)
            (data (i32.const 0) "{{}}") (data (i32.const 100) "bad") (data (i32.const 110) "capture") (data (i32.const 200) "null")
            (func (export "block_alloc") (param i32) (result i32) i32.const 2048)
            (func (export "manifest") (param $ret i32) (result i32)
                local.get $ret i32.const 0 i32.store local.get $ret i32.const 4 i32.add i32.const 2 i32.store i32.const 0)
            (func (export "run") (result i32) (local $status i32)
                {call} local.set $status
                i32.const 110 i32.const 7 i32.const 1024 i32.load i32.const 1028 i32.load i32.const 1040 call $write drop
                local.get $status))"#
        );
        let code = Arc::new(engine.prepare(wat.into_bytes()).await.unwrap());
        for (kind, asynchronous) in (0..3).flat_map(|kind| [(kind, false), (kind, true)]) {
            let host = Host {
                kind,
                captured: None,
            };
            let original = host.error();
            let expected = original.codec_diagnostic().unwrap();
            let decoded = decode_read_response(error_to_response(&original)).unwrap_err();
            assert_eq!(decoded.codec_diagnostic(), Some(expected.clone()));
            let mut run = if asynchronous {
                code.start_async(
                    &supervisor,
                    host,
                    JsonCodec,
                    Format::JSON,
                    ExecutionPolicy::default(),
                )
            } else {
                code.start_sync(
                    &supervisor,
                    host,
                    JsonCodec,
                    Format::JSON,
                    ExecutionPolicy::default(),
                )
            }
            .unwrap();
            let outcome = run.join().await.unwrap();
            assert_eq!(outcome.result.unwrap(), if kind == 1 { -8 } else { -9 });
            let captured = outcome.host.captured.unwrap();
            assert_eq!(
                captured.get(&path!("codec/kind")),
                Some(&Value::String(expected.kind))
            );
            assert!(matches!(
                captured.get(&path!("message")),
                Some(Value::String(_))
            ));
        }
    }
}
