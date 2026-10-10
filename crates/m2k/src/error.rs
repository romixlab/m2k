//! The typed result of talking to an M2K: either `iiod_client`'s own error (the server's errno reaches the
//! caller with its name) or a validation message (a setting out of range, a malformed buffer, ...).

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Iiod(#[from] iiod_client::Error),
    #[error("{0}")]
    Invalid(String),
}

/// `anyhow`-like `.context()`/`.with_context()` on any `Result<T, E: Display>`, folded into [`Error::Invalid`].
pub trait Context<T> {
    fn context(self, msg: impl std::fmt::Display) -> Result<T>;
    fn with_context<F: FnOnce() -> String>(self, f: F) -> Result<T>;
}

impl<T, E: std::fmt::Display> Context<T> for std::result::Result<T, E> {
    fn context(self, msg: impl std::fmt::Display) -> Result<T> {
        self.map_err(|e| Error::Invalid(format!("{msg}: {e}")))
    }

    fn with_context<F: FnOnce() -> String>(self, f: F) -> Result<T> {
        self.map_err(|e| Error::Invalid(format!("{}: {e}", f())))
    }
}

/// Returns an [`Error::Invalid`] with a formatted message, like `anyhow::bail!`.
macro_rules! bail {
    ($($arg:tt)*) => {
        return Err($crate::error::Error::Invalid(format!($($arg)*)))
    };
}
pub(crate) use bail;

/// An [`Error::Invalid`] value with a formatted message, like `anyhow::anyhow!`.
macro_rules! err {
    ($($arg:tt)*) => {
        $crate::error::Error::Invalid(format!($($arg)*))
    };
}
pub(crate) use err;
