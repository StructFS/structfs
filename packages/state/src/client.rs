use crate::*;
use structfs_core_store::{path, Error, Path, Record};
use structfs_serde_store::{from_value, to_value};
use structfs_service::Client;
#[derive(Debug)]
#[non_exhaustive]
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
    /// Submit a command and return its handle without reading the reply.
    async fn submit(&self, command: Command) -> Result<StateHandle, ClientError> {
        let path = self
            .client
            .write(
                &path!("operations"),
                Record::parsed(to_value(&Request::new(command))?),
            )
            .await?;
        // Validate the provider path before the second operation; no global paths.
        if path.len() != 2 || &path[0] != "outstanding" {
            return Err(Error::permission_denied("invalid state handle").into());
        }
        Ok(StateHandle {
            client: self.client.clone(),
            path,
            granted: None,
        })
    }
    /// Submit a command and surface a rejection before returning the handle.
    /// The descriptor read here also pins the handle's epoch, which
    /// [`StateHandle::changes`] checks every caller-supplied token against.
    pub async fn open(&self, command: Command) -> Result<StateHandle, ClientError> {
        let mut handle = self.submit(command).await?;
        match handle.describe().await {
            Ok(descriptor) => {
                handle.granted = Some(descriptor.token);
                Ok(handle)
            }
            Err(e) => {
                let _ = handle.close().await;
                Err(e)
            }
        }
    }
    /// No automatic retry: a transport failure may follow a successful commit.
    ///
    /// Three operations, not four: the receipt is described exactly once,
    /// rather than once by `open` for validation and again for its token.
    pub async fn batch(
        &self,
        expected: Option<Token>,
        mutations: Vec<Mutation>,
    ) -> Result<Token, ClientError> {
        let h = self
            .submit(Command::Batch {
                expected,
                mutations,
            })
            .await?;
        let described = h.describe().await;
        let _ = h.close().await;
        Ok(described?.token)
    }
    /// Conventional store assignment. Null deletes; non-Null values replace the
    /// subtree. The internal batch API can still explicitly store Null. Paths
    /// remain relative to this client's granted view; writes are never retried.
    pub async fn write(&self, path: &Path, value: Value) -> Result<Token, ClientError> {
        let mutation = if value.is_null() {
            Mutation::Delete {
                path: path.to_string(),
            }
        } else {
            Mutation::Set {
                path: path.to_string(),
                value,
            }
        };
        self.batch(None, vec![mutation]).await
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
    /// The descriptor token read when the handle was opened, if it was.
    granted: Option<Token>,
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
        self.read(&cursor_path("snapshot", offset)).await
    }
    /// The next page of changes after `after`.
    ///
    /// Only `after.revision` travels on the wire, so the provider cannot see
    /// which epoch the caller's token came from. The epoch is therefore
    /// checked here, locally, against the epoch this handle was granted at
    /// [`StateClient::open`] — no extra round trip per page.
    pub async fn changes(&self, after: &Token) -> Result<ChangePage, ClientError> {
        let granted = match &self.granted {
            Some(granted) => granted.clone(),
            None => self.describe().await?.token,
        };
        if after.epoch != granted.epoch {
            return Err(ClientError::State(Fault::EpochMismatch {
                current: granted,
            }));
        }
        self.read(&cursor_path("changes", after.revision)).await
    }
    /// Request release of the handle. Idempotent; the provider also reclaims
    /// it when the handle ages out or its owner closes.
    pub async fn close(&self) -> Result<(), ClientError> {
        self.client
            .write(
                &self.path.join(&path!("release")),
                Record::parsed(Value::Null),
            )
            .await?;
        Ok(())
    }
}

/// `{kind}/{cursor}` built from components: a decimal cursor is always a valid
/// component, so there is nothing here that can fail at runtime.
fn cursor_path(kind: &str, cursor: u64) -> Path {
    Path::from_components(vec![kind.to_string(), cursor.to_string()])
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
        let codec =
            structfs_serde_store::ValueCodec::new(structfs_serde_store::CodecProfile::ValueJson)
                .canonical()?;
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
