use crate::*;
use structfs_core_store::{path, Error, Path, Record};
use structfs_serde_store::{from_value, to_value};
use structfs_service::Client;
#[derive(Debug)]
pub enum ClientError {
    Transport(Error),
    State(Fault),
}
impl From<Error> for ClientError {
    fn from(e: Error) -> Self {
        Self::Transport(e)
    }
}
impl std::fmt::Display for ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Transport(e) => e.fmt(f),
            Self::State(e) => e.fmt(f),
        }
    }
}
impl std::error::Error for ClientError {}
/// Bind the supplied client to an Owner with `owned_by` for abandoned-delivery cleanup.
#[derive(Clone)]
pub struct StateClient {
    client: Client,
}
impl StateClient {
    pub fn new(client: Client) -> Self {
        Self { client }
    }
    pub async fn open(&self, command: Command) -> Result<StateHandle, ClientError> {
        let path = self
            .client
            .write(
                &path!("operations"),
                Record::parsed(to_value(&Request::new(command))?),
            )
            .await?;
        let handle = StateHandle {
            client: self.client.clone(),
            path,
        };
        // Validate the provider path before the second operation; no global paths.
        if handle.path.len() != 2 || &handle.path[0] != "outstanding" {
            return Err(Error::permission_denied("invalid state handle").into());
        }
        if let Err(e) = handle.describe().await {
            let _ = handle.release().await;
            return Err(e);
        }
        Ok(handle)
    }
    /// No automatic retry: a transport failure may follow a successful commit.
    pub async fn batch(
        &self,
        expected: Option<Token>,
        mutations: Vec<Mutation>,
    ) -> Result<Token, ClientError> {
        let h = self
            .open(Command::Batch {
                expected,
                mutations,
            })
            .await?;
        let descriptor = h.describe().await?;
        let _ = h.release().await;
        Ok(descriptor.token)
    }
    pub async fn data(&self, path: &Path) -> Result<Option<Value>, ClientError> {
        self.client
            .read(&structfs_core_store::path!("data").join(path))
            .await?
            .map(|r| r.into_value(&structfs_core_store::NoCodec))
            .transpose()
            .map_err(Into::into)
    }
}
/// Server-side ownership and age bounds survive loss of this client object.
/// Explicit release is idempotent. Pages are immutable or cursor-based, so reads
/// can be retried; batch commands must never be retried automatically.
pub struct StateHandle {
    client: Client,
    path: Path,
}
impl StateHandle {
    async fn read<T: serde::de::DeserializeOwned>(&self, suffix: &Path) -> Result<T, ClientError> {
        let r = self
            .client
            .read(&self.path.join(suffix))
            .await?
            .ok_or_else(|| Error::not_found(self.path.clone()))?;
        let reply: Reply<T> = from_value(r.into_value(&structfs_core_store::NoCodec)?)?;
        reply.into_result().map_err(ClientError::State)
    }
    pub async fn describe(&self) -> Result<Descriptor, ClientError> {
        self.read(&path!("")).await
    }
    pub async fn snapshot(&self, offset: u64) -> Result<SnapshotPage, ClientError> {
        self.read(&Path::parse(&format!("snapshot/{offset}")).expect("numeric cursor"))
            .await
    }
    pub async fn changes(&self, after: &Token) -> Result<ChangePage, ClientError> {
        let descriptor = self.describe().await?;
        if after.epoch != descriptor.token.epoch {
            return Err(ClientError::State(Fault::EpochMismatch {
                current: descriptor.token,
            }));
        }
        self.read(&Path::parse(&format!("changes/{}", after.revision)).expect("numeric cursor"))
            .await
    }
    pub async fn release(&self) -> Result<(), ClientError> {
        self.client
            .write(
                &self.path.join(&path!("release")),
                Record::parsed(Value::Null),
            )
            .await?;
        Ok(())
    }
}
impl StateHandle {
    /// Materialize a synchronous projection under explicit client-side bounds.
    pub async fn projection(
        &self,
        max_bytes: usize,
        max_nodes: usize,
    ) -> Result<Projection, ClientError> {
        use structfs_core_store::Codec;
        let descriptor = self.describe().await?;
        let mut nodes = vec![];
        let mut cursor = 0;
        let mut bytes = 0usize;
        let codec = structfs_serde_store::ValueCodec::new(structfs_serde_store::Profile::ValueJson)
            .canonical();
        loop {
            let page = self.snapshot(cursor).await?;
            if page.token != descriptor.token
                || page.next != cursor + page.items.len() as u64
                || (!page.done && page.next == cursor)
            {
                return Err(Error::conflict("inconsistent snapshot page").into());
            }
            for node in page.items {
                let n = codec
                    .encode(&to_value(&node)?, &codec.profile.format())?
                    .len();
                if nodes.len() >= max_nodes || n > max_bytes.saturating_sub(bytes) {
                    return Err(Error::resource_limit("projection bounds").into());
                }
                bytes += n;
                nodes.push(node);
            }
            cursor = page.next;
            if page.done {
                break;
            }
        }
        Projection::from_nodes(descriptor.token, nodes).map_err(ClientError::State)
    }
}
