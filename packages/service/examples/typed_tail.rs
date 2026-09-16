//! Run with `cargo run -p structfs-service --example typed_tail`.
//! Two replay readers acknowledge only after delivery. The coordinator advances
//! retention to their minimum cursor. Disconnect removes a reader explicitly;
//! it must not silently evict unread items from another reader's history.
use std::{collections::BTreeMap, io::Write, time::Duration};
use structfs_service::{CancelToken, CleanupSupervisor, OwnedTail, OwnerLimits};
#[derive(serde::Serialize, serde::Deserialize)]
struct Event {
    text: String,
}
struct Bounded(Vec<u8>, usize);
impl Write for Bounded {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > self.1.saturating_sub(self.0.len()) {
            return Err(std::io::Error::other("encoded page exceeds budget"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let supervisor = CleanupSupervisor::new(1)?;
    let owner = supervisor.owner(OwnerLimits::default())?;
    let tail = OwnedTail::new(&owner.handle(), 2, 512)?;
    let batch = ["first", "last"]
        .into_iter()
        .map(|text| serde_json::to_vec(&Event { text: text.into() }))
        .collect::<Result<Vec<_>, _>>()?;
    tail.push_batch(batch)?; // atomic: an oversized batch publishes nothing
    assert!(tail.push(b"full".to_vec()).is_err()); // producer sees backpressure
    tail.finish(); // final status; existing history remains readable
    let mut acknowledgements = BTreeMap::from([("fast", 0), ("slow", 0)]);
    for reader in ["fast", "slow"] {
        let page = tail.read_bounded(0, 2, 512, &CancelToken::new()).await?;
        let events = page
            .items
            .iter()
            .map(|b| serde_json::from_slice::<Event>(b))
            .collect::<Result<Vec<_>, _>>()?;
        let mut response = Bounded(Vec::new(), 1024);
        serde_json::to_writer(
            &mut response,
            &serde_json::json!({"items":events,"next":page.next,"done":page.done}),
        )?;
        // A real transport acknowledges only after its delivery contract succeeds.
        assert!(!response.0.is_empty());
        assert!(page.done);
        acknowledgements.insert(reader, page.next);
        tail.acknowledge(*acknowledgements.values().min().unwrap())?;
    }
    assert!(owner.close(Duration::from_secs(1)).await.is_quiescent());
    Ok(())
}
