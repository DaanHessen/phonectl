//! Async client for the Android Debug Bridge.
//!
//! Talks to `adbd` on the phone directly (no `adb` server): message framing,
//! authentication, TLS for wireless debugging, pairing and discovery. Knows
//! nothing about what runs on top of the streams it opens.

pub mod auth;
pub mod error;
pub mod message;
pub mod tls;
pub mod transport;

pub use auth::HostKey;
pub use error::Error;
pub use message::{Command, Header, Message};
pub use transport::{Banner, Connection, Negotiated, Stream, negotiate};
