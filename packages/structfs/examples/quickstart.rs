use structfs::{path, MemoryStore, Reader, Record, Value, Writer};

fn main() -> Result<(), structfs::Error> {
    let mut store = MemoryStore::new();
    store.write(
        &path!("greeting"),
        Record::parsed(Value::String("hello".into())),
    )?;
    assert!(store.read(&path!("greeting"))?.is_some());
    Ok(())
}
