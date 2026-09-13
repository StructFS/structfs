#![cfg(feature = "async")]
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use structfs_core_store::{
    path, Cascade, CodecErrorKind, DetachedFuture, DetachedReader, DetachedShared, DetachedWriter,
    Error, Format, Masked, Path, PathPattern, ReadOnly, Record, Rooted, Value,
};
use structfs_serde_store::{DetachedTypedReader, DetachedTypedWriter, JsonCodec};

struct Broker {
    receiver: Option<tokio::sync::oneshot::Receiver<()>>,
    sender: Option<tokio::sync::oneshot::Sender<()>>,
}
impl DetachedReader for Broker {
    fn read_detached(&mut self, from: &Path) -> DetachedFuture<Option<Record>> {
        if from == &path!("scope/parked") {
            let receiver = self.receiver.take().unwrap();
            Box::pin(async move {
                receiver.await.unwrap();
                Ok(Some(Record::parsed(Value::from(7i64))))
            })
        } else {
            Box::pin(async { Ok(Some(Record::parsed(Value::from(8i64)))) })
        }
    }
}
impl DetachedWriter for Broker {
    fn write_detached(&mut self, to: &Path, _: Record) -> DetachedFuture<Path> {
        let sender = self.sender.take().unwrap();
        let path = to.clone();
        Box::pin(async move {
            sender.send(()).unwrap();
            Ok(path)
        })
    }
}
fn send_static<T: Send + 'static>(value: T) -> T {
    value
}

#[tokio::test]
async fn two_reads_release_all_borrows_and_parked_read_does_not_block_write() {
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let mut store = Rooted::new(
        path!("scope"),
        DetachedShared::new(Broker {
            receiver: Some(receiver),
            sender: Some(sender),
        }),
    );
    let mut first = send_static(store.read_typed_detached::<i64>(&path!("parked")));
    let second = send_static(store.read_typed_detached::<i64>(&path!("other")));
    // Actually poll the parked operation before starting the unrelated write.
    assert!(
        std::future::poll_fn(|cx| std::task::Poll::Ready(first.as_mut().poll(cx)))
            .await
            .is_pending()
    );
    let write = {
        let value = 9i64;
        send_static(store.write_typed_detached(&path!("wake"), &value))
    };
    drop(store);
    assert_eq!(write.await.unwrap(), path!("wake"));
    assert_eq!(second.await.unwrap(), Some(8));
    assert_eq!(first.await.unwrap(), Some(7));
}

struct Probe {
    reads: Arc<AtomicUsize>,
    writes: Arc<AtomicUsize>,
    result: Result<Option<Record>, Error>,
    escape: bool,
}
impl DetachedReader for Probe {
    fn read_detached(&mut self, _: &Path) -> DetachedFuture<Option<Record>> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        let result = self
            .result
            .as_ref()
            .map(Clone::clone)
            .map_err(|e| Error::permission_denied(e.to_string()));
        Box::pin(async move { result })
    }
}
impl DetachedWriter for Probe {
    fn write_detached(&mut self, to: &Path, _: Record) -> DetachedFuture<Path> {
        self.writes.fetch_add(1, Ordering::SeqCst);
        let result = if self.escape {
            path!("elsewhere")
        } else {
            to.clone()
        };
        Box::pin(async move { Ok(result) })
    }
}
fn probe(result: Result<Option<Record>, Error>) -> Probe {
    Probe {
        reads: Arc::default(),
        writes: Arc::default(),
        result,
        escape: false,
    }
}
#[tokio::test]
async fn cascade_only_constructs_fallback_after_primary_miss() {
    for primary_result in [
        Ok(None),
        Ok(Some(Record::parsed(Value::Null))),
        Err(Error::permission_denied("primary")),
    ] {
        let fallback = probe(Ok(Some(Record::parsed(Value::from(42i64)))));
        let count = fallback.reads.clone();
        let miss = matches!(&primary_result, Ok(None));
        let failure = primary_result.is_err();
        let mut cascade = Cascade::new(probe(primary_result), DetachedShared::new(fallback));
        let operation = cascade.read_detached(&path!("key"));
        assert_eq!(count.load(Ordering::SeqCst), 0);
        drop(cascade);
        assert_eq!(operation.await.is_err(), failure);
        assert_eq!(count.load(Ordering::SeqCst), usize::from(miss));
    }
}
#[tokio::test]
async fn wrappers_preserve_effect_and_result_boundaries() {
    let inner = probe(Ok(None));
    let writes = inner.writes.clone();
    let mut readonly = ReadOnly::new(inner);
    let operation = readonly.write_detached(&path!("key"), Record::parsed(Value::Null));
    assert_eq!(writes.load(Ordering::SeqCst), 0);
    assert!(matches!(
        operation.await,
        Err(Error::PermissionDenied { .. })
    ));
    let mut inner = probe(Ok(None));
    inner.escape = true;
    let mut rooted = Rooted::new(path!("scope"), inner);
    assert!(rooted
        .write_detached(&path!("key"), Record::parsed(Value::Null))
        .await
        .unwrap_err()
        .to_string()
        .contains("outside root"));
    for result in [
        Ok(None),
        Ok(Some(Record::parsed(Value::from("secret")))),
        Err(Error::permission_denied("no")),
    ] {
        let expected = result.as_ref().map(|r| r.is_some()).map_err(|_| ());
        let mut masked = Masked::new(probe(result), vec![PathPattern::prefix(path!("secret"))]);
        let result = masked.read_detached(&path!("secret/key")).await;
        assert_eq!(
            result.as_ref().map(|r| r.is_some()).map_err(|_| ()),
            expected
        );
        if let Ok(Some(record)) = result {
            assert_eq!(record.as_value(), Some(&Value::from("[masked]")));
        }
    }
}
#[tokio::test]
async fn typed_helpers_preserve_raw_validation_and_do_not_start_invalid_writes() {
    let mut raw = probe(Ok(Some(Record::raw(b"7".to_vec(), Format::JSON))));
    assert_eq!(
        raw.read_as_detached::<i64>(&path!("key"), Arc::new(JsonCodec))
            .await
            .unwrap(),
        Some(7)
    );
    assert!(raw.read_typed_detached::<i64>(&path!("key")).await.is_err());
    let mut malformed = probe(Ok(Some(Record::raw(b"{".to_vec(), Format::JSON))));
    assert!(matches!(
        malformed
            .read_as_detached::<i64>(&path!("key"), Arc::new(JsonCodec))
            .await,
        Err(Error::Codec { .. })
    ));
    let mut parsed = probe(Ok(Some(Record::parsed(Value::from("text")))));
    assert!(matches!(
        parsed.read_typed_detached::<i64>(&path!("key")).await,
        Err(Error::Codec {
            kind: CodecErrorKind::TypeMismatch,
            ..
        })
    ));
    let mut store = probe(Ok(None));
    let count = store.writes.clone();
    let invalid = std::collections::BTreeMap::from([(1i64, "value")]);
    assert!(store
        .write_as_detached(&path!("key"), &invalid)
        .await
        .is_err());
    assert_eq!(count.load(Ordering::SeqCst), 0);
}
