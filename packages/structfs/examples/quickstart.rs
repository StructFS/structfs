use structfs::{path, InMemoryStore, Reader, Record, Value, Writer};

fn main() -> Result<(), structfs::Error> {
    let mut store = InMemoryStore::new();
    store.write(
        &path!("greeting"),
        Record::parsed(Value::String("hello".into())),
    )?;
    assert!(store.read(&path!("greeting"))?.is_some());
    Ok(())
}
