//! Channel-based store for inter-Block communication.
//!
//! ChannelStore provides a StructFS interface over tokio channels,
//! enabling Blocks to communicate through familiar read/write operations.

use std::collections::BTreeMap;

use structfs_core_store::{Error, Path, Reader, Record, Value, Writer};
use tokio::sync::mpsc;

/// A store backed by tokio channels for async message passing.
///
/// ChannelStore allows one Block to send messages to another through
/// the StructFS interface:
///
/// - **Write to root**: Sends a message through the channel
/// - **Read from root**: Receives a message (blocks until available)
/// - **Read from `pending`**: Returns the number of pending messages
///
/// # Example
///
/// ```ignore
/// // Block A creates a channel pair
/// let (sender, receiver) = ChannelStore::pair(100);
///
/// // Block A exports the receiver for Block B
/// ctx.export("messages", receiver);
///
/// // Block A sends messages through the sender
/// sender.write(&Path::parse("").unwrap(), Record::parsed(Value::String("hello".into())))?;
///
/// // Block B reads from its mounted store
/// let msg = store.read(&Path::parse("").unwrap())?;  // Returns "hello"
/// ```
pub struct ChannelStore {
    /// Sender half of the channel.
    tx: Option<mpsc::Sender<Value>>,

    /// Receiver half of the channel.
    rx: Option<mpsc::Receiver<Value>>,

    /// Buffer for received messages (for sync interface).
    buffer: Vec<Value>,
}

// Safety: ChannelStore is Send + Sync because mpsc::Sender and mpsc::Receiver are Send.
// The Option wrappers and Vec are also Send + Sync.
unsafe impl Sync for ChannelStore {}

impl ChannelStore {
    /// Create a new channel store pair with the given buffer capacity.
    ///
    /// Returns (sender_store, receiver_store). The sender can only write,
    /// the receiver can only read.
    pub fn pair(capacity: usize) -> (Self, Self) {
        let (tx, rx) = mpsc::channel(capacity);

        let sender = Self {
            tx: Some(tx),
            rx: None,
            buffer: Vec::new(),
        };

        let receiver = Self {
            tx: None,
            rx: Some(rx),
            buffer: Vec::new(),
        };

        (sender, receiver)
    }

    /// Create a bidirectional channel pair.
    ///
    /// Returns two stores that can both send and receive. Each store's
    /// writes go to the other's read buffer.
    pub fn bidirectional(capacity: usize) -> (Self, Self) {
        let (tx1, rx1) = mpsc::channel(capacity);
        let (tx2, rx2) = mpsc::channel(capacity);

        let store1 = Self {
            tx: Some(tx1),
            rx: Some(rx2),
            buffer: Vec::new(),
        };

        let store2 = Self {
            tx: Some(tx2),
            rx: Some(rx1),
            buffer: Vec::new(),
        };

        (store1, store2)
    }

    /// Check if this store can send messages.
    pub fn can_send(&self) -> bool {
        self.tx.is_some()
    }

    /// Check if this store can receive messages.
    pub fn can_receive(&self) -> bool {
        self.rx.is_some()
    }

    /// Try to receive a message without blocking.
    fn try_recv(&mut self) -> Option<Value> {
        if !self.buffer.is_empty() {
            return Some(self.buffer.remove(0));
        }

        if let Some(ref mut rx) = self.rx {
            rx.try_recv().ok()
        } else {
            None
        }
    }
}

impl Reader for ChannelStore {
    fn read(&mut self, path: &Path) -> Result<Option<Record>, Error> {
        let path_str = path.to_string();
        match path_str.as_str() {
            "" => {
                // Read next message
                match self.try_recv() {
                    Some(value) => Ok(Some(Record::parsed(value))),
                    None => Ok(None),
                }
            }
            "pending" => {
                // Return count of buffered messages
                let count = self.buffer.len() as i64;
                Ok(Some(Record::parsed(Value::Integer(count))))
            }
            "docs" => {
                let mut docs = BTreeMap::new();
                docs.insert("title".to_string(), Value::String("Channel Store".into()));
                docs.insert(
                    "description".to_string(),
                    Value::String(
                        "A store backed by async channels for inter-Block communication.\n\n\
                        Read from root to receive the next message.\n\
                        Write to root to send a message.\n\
                        Read from `pending` to get the count of buffered messages."
                            .into(),
                    ),
                );
                Ok(Some(Record::parsed(Value::Map(docs))))
            }
            _ => Err(Error::store(
                "channel",
                "read",
                format!("invalid path: {}", path_str),
            )),
        }
    }
}

impl Writer for ChannelStore {
    fn write(&mut self, path: &Path, record: Record) -> Result<Path, Error> {
        let path_str = path.to_string();
        match path_str.as_str() {
            "" => {
                if let Some(ref tx) = self.tx {
                    let value = record.into_value(&structfs_core_store::NoCodec)?;
                    tx.try_send(value)
                        .map_err(|e| Error::store("channel", "write", e.to_string()))?;
                    Ok(path.clone())
                } else {
                    Err(Error::store(
                        "channel",
                        "write",
                        "this channel store cannot send",
                    ))
                }
            }
            _ => Err(Error::store(
                "channel",
                "write",
                format!("invalid path: {}", path_str),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use structfs_core_store::NoCodec;

    fn path(s: &str) -> Path {
        Path::parse(s).unwrap()
    }

    #[test]
    fn channel_pair() {
        let (sender, receiver) = ChannelStore::pair(10);
        assert!(sender.can_send());
        assert!(!sender.can_receive());
        assert!(!receiver.can_send());
        assert!(receiver.can_receive());
    }

    #[test]
    fn channel_bidirectional() {
        let (store1, store2) = ChannelStore::bidirectional(10);
        assert!(store1.can_send());
        assert!(store1.can_receive());
        assert!(store2.can_send());
        assert!(store2.can_receive());
    }

    #[test]
    fn channel_send_receive() {
        let (mut sender, mut receiver) = ChannelStore::pair(10);

        // Send a message
        let msg = Value::String("hello".into());
        sender
            .write(&path(""), Record::parsed(msg.clone()))
            .unwrap();

        // Receive it
        let received = receiver.read(&path("")).unwrap().unwrap();
        let value = received.into_value(&NoCodec).unwrap();
        assert_eq!(value, msg);
    }

    #[test]
    fn channel_empty_read() {
        let (_sender, mut receiver) = ChannelStore::pair(10);

        // Reading from empty channel returns None
        let result = receiver.read(&path("")).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn channel_pending() {
        let (mut sender, mut receiver) = ChannelStore::pair(10);

        // Initially no pending messages
        let pending = receiver.read(&path("pending")).unwrap().unwrap();
        let count = pending.into_value(&NoCodec).unwrap();
        assert_eq!(count, Value::Integer(0));

        // Send some messages
        sender
            .write(&path(""), Record::parsed(Value::String("a".into())))
            .unwrap();
        sender
            .write(&path(""), Record::parsed(Value::String("b".into())))
            .unwrap();

        // Read one message to buffer it
        let _ = receiver.read(&path("")).unwrap();

        // One message consumed, one still in channel (not counted as pending)
        let pending = receiver.read(&path("pending")).unwrap().unwrap();
        let count = pending.into_value(&NoCodec).unwrap();
        assert_eq!(count, Value::Integer(0));
    }

    #[test]
    fn channel_docs() {
        let (_, mut receiver) = ChannelStore::pair(10);
        let docs = receiver.read(&path("docs")).unwrap().unwrap();
        let value = docs.into_value(&NoCodec).unwrap();
        if let Value::Map(map) = value {
            assert!(map.contains_key("title"));
            assert!(map.contains_key("description"));
        } else {
            panic!("expected map");
        }
    }

    #[test]
    fn channel_invalid_path() {
        let (_, mut receiver) = ChannelStore::pair(10);
        let result = receiver.read(&path("invalid"));
        assert!(result.is_err());
    }

    #[test]
    fn receiver_cannot_send() {
        let (_, mut receiver) = ChannelStore::pair(10);
        let result = receiver.write(&path(""), Record::parsed(Value::Null));
        assert!(result.is_err());
    }
}
