use std::io;

use crate::message::Command;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("i/o error: {0}")]
    Io(#[from] io::Error),

    #[error("bad message magic for command {command:#010x}")]
    BadMagic { command: u32 },

    #[error("unknown message command {0:#010x}")]
    UnknownCommand(u32),

    #[error("payload of {len} bytes exceeds the negotiated maximum of {max}")]
    PayloadTooLarge { len: usize, max: usize },

    #[error("payload checksum mismatch: header says {expected:#x}, payload sums to {actual:#x}")]
    Checksum { expected: u32, actual: u32 },

    #[error("key error: {0}")]
    Key(String),

    #[error("protocol error: {0}")]
    Protocol(String),

    #[error("tls error: {0}")]
    Tls(String),

    #[error("the connection to the device is closed")]
    ConnectionClosed,

    #[error("the device refused to open the service")]
    ServiceRefused,

    #[error("unexpected {got:?} message while waiting for {expected}")]
    Unexpected { got: Command, expected: &'static str },
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
