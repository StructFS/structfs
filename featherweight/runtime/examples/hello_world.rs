//! Hello World example demonstrating two-Block communication.
//!
//! This example shows how Blocks communicate through StructFS:
//!
//! 1. **Greeting Service Block**: Exports a store that responds to greeting requests
//! 2. **Client Block**: Sends a name to the greeting service and reads the response
//!
//! All communication happens through read/write operations - the client doesn't
//! know it's talking to another Block.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex as StdMutex};

use async_trait::async_trait;
use featherweight_runtime::{Block, BlockContext, BlockId, Result};
use structfs_core_store::{Error, NoCodec, Path, Reader, Record, Value, Writer};

/// A simple in-memory store for inter-Block communication.
///
/// This is a synchronous store using std::sync::Mutex for the example.
struct InMemoryStore {
    data: Arc<StdMutex<BTreeMap<String, Value>>>,
}

impl InMemoryStore {
    fn shared(data: Arc<StdMutex<BTreeMap<String, Value>>>) -> Self {
        Self { data }
    }
}

impl Reader for InMemoryStore {
    fn read(&mut self, path: &Path) -> std::result::Result<Option<Record>, Error> {
        let path_str = path.to_string();
        let data = self.data.lock().unwrap();
        Ok(data.get(&path_str).cloned().map(Record::parsed))
    }
}

impl Writer for InMemoryStore {
    fn write(&mut self, path: &Path, record: Record) -> std::result::Result<Path, Error> {
        let path_str = path.to_string();
        let value = record.into_value(&NoCodec)?;
        let mut data = self.data.lock().unwrap();
        data.insert(path_str, value);
        Ok(path.clone())
    }
}

/// A simple store that prefixes all paths.
struct PrefixedStore {
    prefix: String,
    inner: InMemoryStore,
}

impl PrefixedStore {
    fn new(prefix: &str, data: Arc<StdMutex<BTreeMap<String, Value>>>) -> Self {
        Self {
            prefix: prefix.to_string(),
            inner: InMemoryStore::shared(data),
        }
    }
}

impl Reader for PrefixedStore {
    fn read(&mut self, path: &Path) -> std::result::Result<Option<Record>, Error> {
        let full_path = format!("{}/{}", self.prefix, path);
        let full_path = Path::parse(&full_path).unwrap();
        self.inner.read(&full_path)
    }
}

impl Writer for PrefixedStore {
    fn write(&mut self, path: &Path, record: Record) -> std::result::Result<Path, Error> {
        let full_path = format!("{}/{}", self.prefix, path);
        let full_path = Path::parse(&full_path).unwrap();
        self.inner.write(&full_path, record)
    }
}

/// A simple greeting service that responds with "Hello, {name}!".
struct GreetingService;

#[async_trait]
impl Block<PrefixedStore> for GreetingService {
    async fn run(&mut self, mut ctx: BlockContext<PrefixedStore>) -> Result<()> {
        println!("[GreetingService] Starting...");

        // Check for a request (the store is prefixed to "services/greeter")
        let req_path = Path::parse("request").unwrap();
        if let Some(record) = ctx.root.read(&req_path)? {
            let value = record.into_value(&NoCodec)?;
            if let Value::String(name) = value {
                println!("[GreetingService] Received request: {}", name);

                // Create the greeting
                let greeting = format!("Hello, {}!", name);

                // Write the response
                let resp_path = Path::parse("response").unwrap();
                ctx.root.write(
                    &resp_path,
                    Record::parsed(Value::String(greeting.clone())),
                )?;

                println!("[GreetingService] Sent response: {}", greeting);
            }
        }

        println!("[GreetingService] Done.");
        Ok(())
    }
}

/// A client that sends a greeting request and reads the response.
struct Client {
    name: String,
}

#[async_trait]
impl Block<PrefixedStore> for Client {
    async fn run(&mut self, mut ctx: BlockContext<PrefixedStore>) -> Result<()> {
        println!("[Client] Starting...");

        // Send our name to the greeting service
        println!("[Client] Sending name: {}", self.name);
        let req_path = Path::parse("request").unwrap();
        ctx.root.write(
            &req_path,
            Record::parsed(Value::String(self.name.clone())),
        )?;

        println!("[Client] Done sending request.");
        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    println!("=== Featherweight Hello World ===\n");
    println!("This example demonstrates two Blocks communicating through StructFS.\n");

    // Create shared storage for inter-Block communication
    let shared_data: Arc<StdMutex<BTreeMap<String, Value>>> =
        Arc::new(StdMutex::new(BTreeMap::new()));

    // Create the service's root store (prefixed to see its own namespace)
    let service_store = PrefixedStore::new("services/greeter", shared_data.clone());

    // Create the client's root store (also prefixed to "services/greeter")
    let client_store = PrefixedStore::new("services/greeter", shared_data.clone());

    // Create the Blocks
    let mut client = Client {
        name: "World".to_string(),
    };
    let mut service = GreetingService;

    // Run the client first to send the request
    let client_ctx = BlockContext::new(BlockId::new(), client_store);
    client.run(client_ctx).await?;

    // Then run the service to process it
    let service_ctx = BlockContext::new(BlockId::new(), service_store);
    service.run(service_ctx).await?;

    // Read the response from shared storage
    let data = shared_data.lock().unwrap();
    if let Some(Value::String(greeting)) = data.get("services/greeter/response") {
        println!("\n[Main] Final greeting from service: {}", greeting);
    }

    println!("\n=== Example complete ===");
    Ok(())
}
