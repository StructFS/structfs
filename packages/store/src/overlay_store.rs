use serde::de::{DeserializeOwned, Deserializer};
use serde::Serialize;

use crate::store::{
    Error as StoreError, ObjectSafeStore, Path, PathError, Reader as StoreRead, Store,
    Writer as StoreWrite,
};

#[derive(thiserror::Error, Debug, PartialEq, Eq)]
pub enum Error {
    #[error("The path {path:?} could not be routed.")]
    NoRouteFoundForPath { path: Path },
}

pub struct OverlayMaskStore {
    _private: (),
}
impl OverlayMaskStore {
    fn new() -> Self {
        OverlayMaskStore { _private: () }
    }

    fn deserializer<'de, RecordSource: Deserializer<'de>>(
        _from: &Path,
    ) -> Result<Option<RecordSource>, StoreError> {
        Ok(None)
    }

    fn read<RecordType: DeserializeOwned>(_from: &Path) -> Result<Option<RecordType>, StoreError> {
        Ok(None)
    }

    fn write(to: &Path) -> Result<Path, StoreError> {
        Err(StoreError::from(PathError::PathNotWritable {
            path: to.clone(),
            message: "unknown error".to_string(),
        }))
    }
}

impl StoreRead for OverlayMaskStore {
    fn read_to_deserializer<'de, 'this>(
        &'this mut self,
        from: &Path,
    ) -> Result<Option<Box<dyn erased_serde::Deserializer<'de>>>, StoreError>
    where
        'this: 'de,
    {
        Self::deserializer(from)
    }

    fn read_owned<RecordType: DeserializeOwned>(
        &mut self,
        from: &Path,
    ) -> Result<Option<RecordType>, StoreError> {
        Self::read(from)
    }
}

impl StoreWrite for OverlayMaskStore {
    fn write<D: Serialize>(&mut self, to: &Path, _data: D) -> Result<Path, StoreError> {
        Self::write(to)
    }
}

pub struct OnlyWritable<'sw, SW: StoreWrite + 'sw> {
    writable: SW,
    _lifetime: std::marker::PhantomData<&'sw ()>,
}

impl<'sw, SW: StoreWrite + 'sw> OnlyWritable<'sw, SW> {
    pub fn wrap(writable: SW) -> Self {
        OnlyWritable {
            writable,
            _lifetime: std::marker::PhantomData,
        }
    }
}

impl<'sw, SW: StoreWrite + 'sw> StoreWrite for OnlyWritable<'sw, SW> {
    fn write<D: Serialize>(&mut self, to: &Path, data: D) -> Result<Path, StoreError> {
        self.writable.write(to, data)
    }
}

impl<'sw, SW: StoreWrite + 'sw> StoreRead for OnlyWritable<'sw, SW> {
    fn read_to_deserializer<'de, 'this>(
        &'this mut self,
        from: &Path,
    ) -> Result<Option<Box<dyn erased_serde::Deserializer<'de>>>, StoreError>
    where
        'this: 'de,
    {
        OverlayMaskStore::deserializer(from)
    }

    fn read_owned<RecordType: DeserializeOwned>(
        &mut self,
        from: &Path,
    ) -> Result<Option<RecordType>, StoreError> {
        OverlayMaskStore::read(from)
    }
}

pub struct OnlyReadable<SR: StoreRead> {
    readable: SR,
}

impl<SR: StoreRead> OnlyReadable<SR> {
    pub fn wrap(readable: SR) -> Self {
        OnlyReadable { readable }
    }
}

impl<SR: StoreRead> StoreWrite for OnlyReadable<SR> {
    fn write<D: Serialize>(&mut self, to: &Path, _data: D) -> Result<Path, StoreError> {
        OverlayMaskStore::write(to)
    }
}

impl<SR: StoreRead> StoreRead for OnlyReadable<SR> {
    fn read_to_deserializer<'de, 'this>(
        &'this mut self,
        from: &Path,
    ) -> Result<Option<Box<dyn erased_serde::Deserializer<'de>>>, StoreError>
    where
        'this: 'de,
    {
        self.readable.read_to_deserializer(from)
    }

    fn read_owned<RecordType: DeserializeOwned>(
        &mut self,
        from: &Path,
    ) -> Result<Option<RecordType>, StoreError> {
        self.readable.read_owned(from)
    }
}

pub struct SubStoreView<S> {
    wrapped: S,
    sub_path: Path,
}

impl<S> SubStoreView<S> {
    pub fn wrap(viewable: S, sub_path: Path) -> SubStoreView<S> {
        SubStoreView {
            wrapped: viewable,
            sub_path,
        }
    }
}

impl<SW: StoreWrite> StoreWrite for SubStoreView<SW> {
    fn write<D: Serialize>(&mut self, to: &Path, data: D) -> Result<Path, StoreError> {
        // TODO(alex): This could probably be done faster... e.g. in a way that avoids two clones
        // for every call.
        self.wrapped.write(&self.sub_path.join(to), data)
    }
}

impl<SR: StoreRead> StoreRead for SubStoreView<SR> {
    fn read_to_deserializer<'de, 'this>(
        &'this mut self,
        from: &Path,
    ) -> Result<Option<Box<dyn erased_serde::Deserializer<'de>>>, StoreError>
    where
        'this: 'de,
    {
        // TODO(alex): This could probably be done faster... e.g. in a way that avoids two clones
        // for every call.
        self.wrapped.read_to_deserializer(&self.sub_path.join(from))
    }

    fn read_owned<RecordType: DeserializeOwned>(
        &mut self,
        from: &Path,
    ) -> Result<Option<RecordType>, StoreError> {
        // TODO(alex): This could probably be done faster... e.g. in a way that avoids two clones
        // for every call.
        self.wrapped.read_owned(&self.sub_path.join(from))
    }
}

pub struct StoreWriteReturnPathRewriter<SW, RW: Fn(&Path, Path) -> Path> {
    pub wrapped: SW,
    pub rewriter: RW,
}

impl<SW: StoreWrite, RW: Fn(&Path, Path) -> Path> StoreWrite
    for StoreWriteReturnPathRewriter<SW, RW>
{
    fn write<D: Serialize>(&mut self, to: &Path, data: D) -> Result<Path, StoreError> {
        self.wrapped
            .write(to, data)
            .map(|returned| (self.rewriter)(to, returned))
    }
}

impl<SR: StoreRead, RW: Fn(&Path, Path) -> Path> StoreRead
    for StoreWriteReturnPathRewriter<SR, RW>
{
    fn read_to_deserializer<'de, 'this>(
        &'this mut self,
        from: &Path,
    ) -> Result<Option<Box<dyn erased_serde::Deserializer<'de>>>, StoreError>
    where
        'this: 'de,
    {
        self.wrapped.read_to_deserializer(from)
    }

    fn read_owned<RecordType: DeserializeOwned>(
        &mut self,
        from: &Path,
    ) -> Result<Option<RecordType>, StoreError> {
        self.wrapped.read_owned(from)
    }
}

pub struct StoreBox<'sbox> {
    store: Box<dyn ObjectSafeStore + 'sbox + Send + Sync>,
}

impl<'sbox> StoreBox<'sbox> {
    pub fn new<S: Store + 'sbox + Send + Sync>(inner: S) -> StoreBox<'sbox> {
        StoreBox {
            store: Box::new(<dyn ObjectSafeStore>::erase(inner)),
        }
    }
}

impl<'sbox> StoreRead for StoreBox<'sbox> {
    fn read_to_deserializer<'de, 'this>(
        &'this mut self,
        from: &Path,
    ) -> Result<Option<Box<dyn erased_serde::Deserializer<'de>>>, StoreError>
    where
        'this: 'de,
    {
        let mut maybe_deserializer: Option<Box<dyn erased_serde::Deserializer<'de>>> = None;
        {
            let mut callback = |maybe_erased: Option<Box<dyn erased_serde::Deserializer<'de>>>| {
                if let Some(erased) = maybe_erased {
                    let _ = maybe_deserializer.insert(erased);
                }

                Ok(())
            };
            self.store
                .object_safe_read_to_deserializer(from, &mut callback)?;
        }

        Ok(maybe_deserializer)
    }

    fn read_owned<RecordType: DeserializeOwned>(
        &mut self,
        from: &Path,
    ) -> Result<Option<RecordType>, StoreError> {
        // Matching prefix to store occurs in `self.read_to_deserializer(...)`.
        Ok(
            if let Some(deserializer) = self.read_to_deserializer(from)? {
                let record = RecordType::deserialize(deserializer).map_err(|error| {
                    StoreError::RecordDeserialization {
                        message: error.to_string(),
                    }
                })?;

                Some(record)
            } else {
                None
            },
        )
    }
}

impl<'sbox> StoreWrite for StoreBox<'sbox> {
    fn write<RecordType: Serialize>(
        &mut self,
        destination: &Path,
        data: RecordType,
    ) -> Result<Path, StoreError> {
        self.store.object_safe_write(destination, &data)
    }
}

// TODO(alex): Create a simple BoxStore that provides a concrete type wrapper for an
// ObjectSafeStore so one doesn't have to use a whole OverlayStore to do the job of provided an
// ergonomic way to accept a generic store.
// TODO(alex): Create an async version of the overlay store
#[derive(Default)]
pub struct OverlayStore<'routes> {
    // TODO(alex): Make a more efficient structure for lookup than an in-order routing list.
    routes: Vec<(Path, Box<dyn ObjectSafeStore + 'routes + Send + Sync>)>,
    _private: (),
}
impl<'os> OverlayStore<'os> {
    // TODO(alex): Eventually support routing reads and writes separately for the same path if
    // desired.  This should be supported by a different store that manages that distinction and
    // then is passed here.
    pub fn add_layer<'s, S: 's + Store + Send + Sync>(
        &mut self,
        mount_root: Path,
        store: S,
    ) -> Result<(), Error>
    where
        's: 'os,
    {
        let r: Box<dyn ObjectSafeStore + Send + Sync + 's> =
            Box::new(<dyn ObjectSafeStore>::erase(store));
        self.routes.push((mount_root, r));
        Ok(())
    }

    pub fn add_write_only_layer<'sw, SW: 'sw + StoreWrite + Send + Sync>(
        &mut self,
        mount_root: Path,
        store: SW,
    ) -> Result<(), Error>
    where
        'sw: 'os,
    {
        self.add_layer(mount_root, OnlyWritable::wrap(store))
    }

    pub fn add_read_only_layer<'sr, SR: 'sr + StoreRead + Send + Sync>(
        &mut self,
        mount_root: Path,
        store: SR,
    ) -> Result<(), Error>
    where
        'sr: 'os,
    {
        self.add_layer(mount_root, OnlyReadable::wrap(store))
    }

    pub fn remove_layer(&mut self, _mount_root: &Path) -> Result<(), Error> {
        panic!("Not yet implemented");
    }

    pub fn mask_sub_tree(&mut self, root: Path) -> Result<(), Error> {
        self.add_layer(root, OverlayMaskStore::new())
    }

    fn match_prefix_to_store(
        &mut self,
        to_match: &Path,
    ) -> Option<(Path, &mut Box<dyn ObjectSafeStore + Send + Sync + 'os>)> {
        for (prefix, store) in self.routes.iter_mut().rev() {
            if to_match.has_prefix(prefix) {
                return Some((to_match.strip_prefix(prefix).unwrap(), store));
            }
        }

        None
    }

    // TODO(alex): Figure out a way to return a clean structured representation of the tree.
    // fn get_sub_tree_configuration(root: &Path) -> Result<(), Error> {
    //     Ok(())
    // }
}

impl<'os> StoreRead for OverlayStore<'os> {
    fn read_to_deserializer<'de, 'this>(
        &'this mut self,
        from: &Path,
    ) -> Result<Option<Box<dyn erased_serde::Deserializer<'de>>>, StoreError>
    where
        'this: 'de,
    {
        if let Some((suffix, store)) = self.match_prefix_to_store(from) {
            let mut maybe_deserializer: Option<Box<dyn erased_serde::Deserializer<'de>>> = None;
            {
                let mut callback =
                    |maybe_erased: Option<Box<dyn erased_serde::Deserializer<'de>>>| {
                        if let Some(erased) = maybe_erased {
                            let _ = maybe_deserializer.insert(erased);
                        }

                        Ok(())
                    };
                store.object_safe_read_to_deserializer(&suffix, &mut callback)?;
            }

            return Ok(maybe_deserializer);
        }

        Err(StoreError::ImplementationFailure {
            message: (Error::NoRouteFoundForPath { path: from.clone() }).to_string(),
        })
    }

    fn read_owned<RecordType: DeserializeOwned>(
        &mut self,
        from: &Path,
    ) -> Result<Option<RecordType>, StoreError> {
        // Matching prefix to store occurs in `self.read_to_deserializer(...)`.
        Ok(
            if let Some(deserializer) = self.read_to_deserializer(from)? {
                let record = RecordType::deserialize(deserializer).map_err(|error| {
                    StoreError::RecordDeserialization {
                        message: error.to_string(),
                    }
                })?;

                Some(record)
            } else {
                None
            },
        )
    }
}

impl<'os> StoreWrite for OverlayStore<'os> {
    fn write<RecordType: Serialize>(
        &mut self,
        destination: &Path,
        data: RecordType,
    ) -> Result<Path, StoreError> {
        for (prefix, store) in self.routes.iter_mut().rev() {
            if destination.has_prefix(prefix) {
                let remainder = destination.strip_prefix(prefix).unwrap();
                return store.object_safe_write(&remainder, &data);
            }
        }
        Err(StoreError::ImplementationFailure {
            message: (Error::NoRouteFoundForPath {
                path: destination.clone(),
            })
            .to_string(),
        })
    }
}
