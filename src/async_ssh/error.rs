use std::error::Error as StdError;
use std::fmt;
use std::io;

#[derive(Debug)]
pub enum Error {
    Ssh2(ssh2::Error),
    Io(io::Error),
    Disconnected,
    Timeout(String),
    Other(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Ssh2(e) => write!(f, "SSH error: {}", e),
            Error::Io(e) => write!(f, "I/O error: {}", e),
            Error::Disconnected => write!(f, "SSH session disconnected"),
            Error::Timeout(msg) => write!(f, "Timeout: {}", msg),
            Error::Other(msg) => write!(f, "{}", msg),
        }
    }
}

impl StdError for Error {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        match self {
            Error::Ssh2(e) => Some(e),
            Error::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<ssh2::Error> for Error {
    fn from(e: ssh2::Error) -> Self {
        Error::Ssh2(e)
    }
}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Error::Io(e)
    }
}

pub type Result<T> = std::result::Result<T, Error>;