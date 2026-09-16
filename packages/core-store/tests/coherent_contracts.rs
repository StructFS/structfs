use structfs_core_store::{path, MemoryStore, Reader, Record, Value, Writer};

#[test]
fn snapshot_construction_and_write_convention_are_distinct() {
    let mut store = MemoryStore::from_entries([
        (path!("settings/example"), Value::Null),
        (path!("empty_map"), Value::map()),
        (path!("empty_array"), Value::Array(vec![])),
    ])
    .unwrap();
    assert_eq!(
        store
            .read(&path!("settings/example"))
            .unwrap()
            .unwrap()
            .as_value(),
        Some(&Value::Null)
    );
    assert_eq!(
        store.read_children(&path!("settings")).unwrap(),
        Some(vec!["example".into()])
    );
    store
        .write(&path!("settings/example"), Record::parsed(Value::Null))
        .unwrap();
    assert!(store.read(&path!("settings/example")).unwrap().is_none());
    assert_eq!(
        store.read_children(&path!("settings")).unwrap(),
        Some(vec![])
    );
    assert!(store.read(&path!("empty_map")).unwrap().is_some());
    assert!(store.read(&path!("empty_array")).unwrap().is_some());
    let mut root = MemoryStore::from_entries([(path!(""), Value::Null)]).unwrap();
    assert_eq!(root.root(), Some(&Value::Null));
    assert!(root.read(&path!("")).unwrap().is_some());
    root.write(&path!(""), Record::parsed(Value::Null)).unwrap();
    assert_eq!(root.root(), None);
    assert_eq!(MemoryStore::from_entries([]).unwrap().root(), None);
}

#[test]
fn snapshots_reject_ambiguous_entries_in_either_order() {
    for paths in [
        [path!("a"), path!("a/b")],
        [path!("a"), path!("a")],
        [path!(""), path!("a")],
    ] {
        for order in [paths.clone(), [paths[1].clone(), paths[0].clone()]] {
            assert!(
                MemoryStore::from_entries(order.into_iter().map(|p| (p, Value::Null))).is_err()
            );
        }
    }
    let mut store = MemoryStore::from_entries([
        (path!("a/0"), Value::Bool(true)),
        (path!("aa"), Value::Bool(false)),
    ])
    .unwrap();
    assert!(store
        .read(&path!("a"))
        .unwrap()
        .unwrap()
        .as_value()
        .unwrap()
        .is_map());
    let value = Value::Map([("not/a/path".into(), Value::Null)].into());
    assert_eq!(MemoryStore::with_root(value.clone()).root(), Some(&value));
}

#[cfg(feature = "async")]
#[tokio::test]
async fn erased_shared_operations_do_not_hold_construction_lock() {
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    use structfs_core_store::{
        DetachedFuture, DetachedReader, DetachedShared, DetachedWriter, SharedReader, SharedWriter,
    };
    struct Provider(Arc<AtomicUsize>);
    impl DetachedReader for Provider {
        fn read_detached(
            &mut self,
            _: &structfs_core_store::Path,
        ) -> DetachedFuture<Option<Record>> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Box::pin(std::future::pending())
        }
    }
    impl DetachedWriter for Provider {
        fn write_detached(
            &mut self,
            p: &structfs_core_store::Path,
            _: Record,
        ) -> DetachedFuture<structfs_core_store::Path> {
            self.0.fetch_add(1, Ordering::SeqCst);
            let p = p.clone();
            Box::pin(async move { Ok(p) })
        }
    }
    let accepted = Arc::new(AtomicUsize::new(0));
    let shared = Arc::new(DetachedShared::new(Provider(accepted.clone())));
    let reader: Arc<dyn SharedReader> = shared.clone();
    let writer: Arc<dyn SharedWriter> = shared;
    let parked = reader.read(path!("pending"));
    let first = writer.write(path!("one"), Record::parsed(Value::Null));
    let second = writer.write(path!("two"), Record::parsed(Value::Null));
    assert_eq!(accepted.load(Ordering::SeqCst), 3);
    assert_eq!(first.await.unwrap(), path!("one"));
    assert_eq!(second.await.unwrap(), path!("two"));
    drop(parked);
}

#[cfg(feature = "async")]
#[tokio::test]
async fn construction_panic_prevents_reentry_through_either_interface() {
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    use structfs_core_store::{
        DetachedFuture, DetachedReader, DetachedShared, DetachedWriter, Path, SharedReader,
        SharedWriter,
    };
    struct Provider(Arc<AtomicUsize>);
    impl DetachedReader for Provider {
        fn read_detached(&mut self, _: &Path) -> DetachedFuture<Option<Record>> {
            self.0.fetch_add(1, Ordering::SeqCst);
            panic!("partially updated provider");
        }
    }
    impl DetachedWriter for Provider {
        fn write_detached(&mut self, _: &Path, _: Record) -> DetachedFuture<Path> {
            self.0.fetch_add(1, Ordering::SeqCst);
            panic!("partially updated provider");
        }
    }
    for panic_on_write in [false, true] {
        let entered = Arc::new(AtomicUsize::new(0));
        let mut provider = DetachedShared::new(Provider(entered.clone()));
        let other = provider.clone();
        assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            if panic_on_write {
                drop(SharedWriter::write(
                    &provider,
                    path!("key"),
                    Record::parsed(Value::Null),
                ));
            } else {
                drop(SharedReader::read(&provider, path!("key")));
            }
        }))
        .is_err());
        assert!(SharedReader::read(&other, path!("key")).await.is_err());
        assert!(
            SharedWriter::write(&other, path!("key"), Record::parsed(Value::Null))
                .await
                .is_err()
        );
        assert!(provider.read_detached(&path!("key")).await.is_err());
        assert!(provider
            .write_detached(&path!("key"), Record::parsed(Value::Null))
            .await
            .is_err());
        assert_eq!(entered.load(Ordering::SeqCst), 1);
    }
}

#[test]
fn lazy_decode_is_serialized_and_failed_initialization_can_retry() {
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Barrier,
    };
    use structfs_core_store::{Codec, Error, Format, LazyRecord};
    struct Counting(AtomicUsize);
    impl Codec for Counting {
        fn supports(&self, _: &Format) -> bool {
            true
        }
        fn decode(&self, _: &bytes::Bytes, _: &Format) -> Result<Value, Error> {
            self.0.fetch_add(1, Ordering::SeqCst);
            std::thread::sleep(std::time::Duration::from_millis(5));
            Ok(Value::Integer(7))
        }
        fn encode(&self, _: &Value, _: &Format) -> Result<bytes::Bytes, Error> {
            unreachable!()
        }
    }
    let record = Arc::new(LazyRecord::from_raw(
        bytes::Bytes::from_static(b"7"),
        Format::JSON,
    ));
    assert!(record.value(&structfs_core_store::NoCodec).is_err());
    let codec = Arc::new(Counting(AtomicUsize::new(0)));
    let barrier = Arc::new(Barrier::new(8));
    let threads: Vec<_> = (0..8)
        .map(|_| {
            let (record, codec, barrier) = (record.clone(), codec.clone(), barrier.clone());
            std::thread::spawn(move || {
                barrier.wait();
                assert_eq!(record.value(codec.as_ref()).unwrap(), &Value::Integer(7));
            })
        })
        .collect();
    for t in threads {
        t.join().unwrap();
    }
    assert_eq!(codec.0.load(Ordering::SeqCst), 1);
}

#[test]
fn child_projection_uses_override_and_distinguishes_missing_leaf_and_pages() {
    use structfs_core_store::{ChildNames, ChildPage, Error, Path};
    struct Raw;
    impl Reader for Raw {
        fn read(&mut self, _: &Path) -> Result<Option<Record>, Error> {
            panic!("must not fetch a record")
        }
        fn read_children_page(
            &mut self,
            p: &Path,
            offset: usize,
            limit: usize,
        ) -> Result<Option<ChildPage>, Error> {
            assert_eq!(p, &path!("huge"));
            assert_eq!(limit, 2);
            Ok(Some(ChildPage {
                names: vec![offset.to_string(), (offset + 1).to_string()],
                next: Some(offset + 2),
            }))
        }
    }
    let mut projection = ChildNames::new(Raw, 2, 10).unwrap();
    assert!(projection.read(&path!("1000000/2/huge")).is_err()); // byte budget
    let page = projection.read(&path!("0/2/huge")).unwrap().unwrap();
    assert_eq!(
        page.as_value().unwrap().get(&path!("next")),
        Some(&Value::Integer(2))
    );
    assert!(projection.read(&path!("0/3/huge")).is_err());
    let mut projection = ChildNames::new(
        MemoryStore::with_root(Value::Array(vec![Value::Null, Value::Integer(1)])),
        2,
        10,
    )
    .unwrap();
    assert!(projection.read(&path!("0/2/missing")).unwrap().is_none());
    let leaf = projection.read(&path!("0/2/0")).unwrap().unwrap();
    assert_eq!(
        leaf.as_value().unwrap().get(&path!("names")),
        Some(&Value::Array(vec![]))
    );
    let array = projection.read(&path!("0/2")).unwrap().unwrap();
    assert_eq!(
        array.as_value().unwrap().get(&path!("names")),
        Some(&Value::Array(vec![Value::from("0"), Value::from("1")]))
    );
}
