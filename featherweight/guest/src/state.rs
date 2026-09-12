//! Same versioned protocol as native clients. Handle cleanup is owned by the
//! host's request/instance, even when a guest traps before explicit release.
use crate::sdk::{self, ValueError};
use structfs_serde_store::{from_value, to_value, Path, Value, ValueCodec};
pub use structfs_state::{
    ChangePage, Command, Descriptor, Fault, Mutation, ReadLimits, Request, SnapshotPage, Token,
};
#[derive(Debug)]
pub enum Error {
    Transport(ValueError),
    State(Fault),
}
impl From<ValueError> for Error {
    fn from(e: ValueError) -> Self {
        Self::Transport(e)
    }
}
/// `root` is the guest-visible grant, with data and operations below it.
pub struct Client {
    root: Path,
    codec: ValueCodec,
}
pub struct Handle<'a> {
    client: &'a Client,
    path: Path,
}
impl Client {
    pub fn new(root: Path, codec: ValueCodec) -> Self {
        Self { root, codec }
    }
    /// Never retry a batch automatically: a failed transport can follow commit.
    pub fn open(&self, command: Command) -> Result<Handle<'_>, Error> {
        let v = to_value(&Request::new(command)).map_err(ValueError::Codec)?;
        let returned = sdk::write_value(
            &self
                .root
                .join(&Path::parse("operations").unwrap())
                .to_string(),
            &v,
            &self.codec,
        )?;
        let path =
            Path::parse(&returned).map_err(|_| ValueError::Host("invalid state handle".into()))?;
        let relative = path
            .strip_prefix(&self.root)
            .ok_or_else(|| ValueError::Host("escaped state handle".into()))?;
        if relative.len() != 2 || &relative[0] != "outstanding" {
            return Err(ValueError::Host("invalid state handle".into()).into());
        }
        let h = Handle { client: self, path };
        if let Err(e) = h.describe() {
            let _ = h.release();
            return Err(e);
        }
        Ok(h)
    }
    pub fn batch(&self, expected: Option<Token>, mutations: Vec<Mutation>) -> Result<Token, Error> {
        let h = self.open(Command::Batch {
            expected,
            mutations,
        })?;
        let token = h.describe()?.token;
        let _ = h.release();
        Ok(token)
    }
}
impl Handle<'_> {
    fn read<T: serde::de::DeserializeOwned>(&self, suffix: &str) -> Result<T, Error> {
        let path = self.path.join(
            &Path::parse(suffix).map_err(|_| ValueError::Host("invalid handle suffix".into()))?,
        );
        let v = sdk::read_value(&path.to_string(), &self.client.codec)?
            .ok_or_else(|| ValueError::Host("missing state handle".into()))?;
        let reply: structfs_state::Reply<T> = from_value(v).map_err(ValueError::Codec)?;
        reply.into_result().map_err(Error::State)
    }
    pub fn describe(&self) -> Result<Descriptor, Error> {
        self.read("")
    }
    pub fn snapshot(&self, offset: u64) -> Result<SnapshotPage, Error> {
        self.read(&format!("snapshot/{offset}"))
    }
    pub fn changes(&self, after: &Token) -> Result<ChangePage, Error> {
        let descriptor = self.describe()?;
        if descriptor.token.epoch != after.epoch {
            return Err(Error::State(Fault::EpochMismatch {
                current: descriptor.token,
            }));
        }
        self.read(&format!("changes/{}", after.revision))
    }
    pub fn release(&self) -> Result<(), Error> {
        sdk::write_value(
            &self.path.join(&Path::parse("release").unwrap()).to_string(),
            &Value::Null,
            &self.client.codec,
        )?;
        Ok(())
    }
}
