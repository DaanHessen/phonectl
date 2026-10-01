//! Everything about the phone that is not wire protocol and not UI: the
//! device model, the link protocol spoken with the Android app, and the ADB
//! connection used for one-time setup.

pub mod connect;
pub mod link;
pub mod paths;

pub use connect::{Connected, ShellOutput, connect, discover_and_connect, shell};
