#![cfg(feature = "async")]
use structfs_core_store::{
    path, AsyncReader, DetachedFuture, DetachedReader, Error, Format, Path, Reader, Record, Value,
};
use structfs_serde_store::{AsyncTypedReader, DetachedTypedReader, TypedReader};
struct Fixture(Option<Record>);
impl Reader for Fixture {
    fn read(&mut self, _: &Path) -> Result<Option<Record>, Error> {
        Ok(self.0.clone())
    }
}
#[async_trait::async_trait]
impl AsyncReader for Fixture {
    async fn read_async(&mut self, _: &Path) -> Result<Option<Record>, Error> {
        Ok(self.0.clone())
    }
}
impl DetachedReader for Fixture {
    fn read_detached(&mut self, _: &Path) -> DetachedFuture<Option<Record>> {
        let record = self.0.clone();
        Box::pin(async { Ok(record) })
    }
}
#[tokio::test]
async fn implicit_helpers_have_identical_record_semantics() {
    for record in [
        None,
        Some(Record::parsed(Value::Integer(1))),
        Some(Record::parsed(Value::Null)),
        Some(Record::raw(bytes::Bytes::from_static(b"1"), Format::JSON)),
        Some(Record::raw(
            bytes::Bytes::from_static(b"1"),
            Format::OCTET_STREAM,
        )),
    ] {
        let mut fixture = Fixture(record);
        let sync = fixture.read_typed::<i64>(&path!("value"));
        let asynchronous = fixture.read_typed_async::<i64>(&path!("value")).await;
        let detached = fixture.read_typed_detached::<i64>(&path!("value")).await;
        assert_eq!(format!("{sync:?}"), format!("{asynchronous:?}"));
        assert_eq!(format!("{sync:?}"), format!("{detached:?}"));
    }
}
