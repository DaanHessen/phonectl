//! Everything about the phone that is not wire protocol and not UI: the
//! device model, capabilities, the agent protocol and the connection
//! lifecycle.

pub mod connect;
pub mod paths;

pub use connect::{Connected, ShellOutput, connect, discover_and_connect, shell};
