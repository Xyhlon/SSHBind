// Custom async SSH wrapper to replace async-ssh2-lite
// Provides same API but with direct control over async implementation

mod error;
mod session;
mod channel;
mod stream;

pub use error::{Error, Result};
pub use session::{AsyncSession, SessionConfiguration};
pub use channel::AsyncChannel;
pub use stream::AsyncTcpStream;

// Re-export ssh2 types that users need
pub use ssh2::{
    KeyboardInteractivePrompt, Prompt,
    DisconnectCode, HashType, HostKeyType, MethodType,
};