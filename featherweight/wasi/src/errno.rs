//! WASI preview1 errno values (the subset the shim produces, plus the
//! type). Numbering per the WASI preview1 specification.

/// A WASI errno.
pub type Errno = u16;

pub const SUCCESS: Errno = 0;
pub const ACCES: Errno = 2;
pub const AGAIN: Errno = 6;
pub const BADF: Errno = 8;
pub const EXIST: Errno = 20;
pub const INTR: Errno = 27;
pub const INVAL: Errno = 28;
pub const IO: Errno = 29;
pub const NOENT: Errno = 44;
pub const NOSYS: Errno = 52;
pub const NOTSUP: Errno = 58;
pub const TIMEDOUT: Errno = 73;
pub const NOTCAPABLE: Errno = 76;
