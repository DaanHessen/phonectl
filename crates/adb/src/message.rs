//! ADB message framing.
//!
//! Every message is a 24-byte little-endian header followed by an optional
//! payload (AOSP `adb/protocol.txt`, `adb.h`):
//!
//! ```text
//! command  arg0  arg1  data_length  data_check  magic (= command ^ 0xffffffff)
//! ```
//!
//! `data_check` is the byte sum of the payload, not a CRC despite its old name.
//! Peers at `A_VERSION_SKIP_CHECKSUM` or later may send zero and skip checking.

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::error::{Error, Result};

pub const HEADER_LEN: usize = 24;

/// Oldest protocol version, which requires payload checksums.
pub const VERSION_MIN: u32 = 0x0100_0000;
/// Protocol version that allows skipping payload checksums. What we send.
pub const VERSION_SKIP_CHECKSUM: u32 = 0x0100_0001;
/// Version carried in STLS messages.
pub const STLS_VERSION: u32 = 0x0100_0000;

/// Payload limit before CNXN negotiates a larger one.
pub const MAX_PAYLOAD_V1: usize = 4 * 1024;
/// Payload limit we offer in CNXN.
pub const MAX_PAYLOAD: usize = 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum Command {
    Sync = 0x434e_5953,
    Cnxn = 0x4e58_4e43,
    Auth = 0x4854_5541,
    Open = 0x4e45_504f,
    Okay = 0x5941_4b4f,
    Clse = 0x4553_4c43,
    Wrte = 0x4554_5257,
    Stls = 0x534c_5453,
}

impl TryFrom<u32> for Command {
    type Error = Error;

    fn try_from(value: u32) -> Result<Self> {
        Ok(match value {
            0x434e_5953 => Self::Sync,
            0x4e58_4e43 => Self::Cnxn,
            0x4854_5541 => Self::Auth,
            0x4e45_504f => Self::Open,
            0x5941_4b4f => Self::Okay,
            0x4553_4c43 => Self::Clse,
            0x4554_5257 => Self::Wrte,
            0x534c_5453 => Self::Stls,
            other => return Err(Error::UnknownCommand(other)),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    pub command: Command,
    pub arg0: u32,
    pub arg1: u32,
    pub data_length: u32,
    pub data_check: u32,
}

impl Header {
    pub fn encode(&self) -> [u8; HEADER_LEN] {
        let command = self.command as u32;
        let mut out = [0u8; HEADER_LEN];
        for (i, word) in [
            command,
            self.arg0,
            self.arg1,
            self.data_length,
            self.data_check,
            command ^ 0xffff_ffff,
        ]
        .into_iter()
        .enumerate()
        {
            out[i * 4..i * 4 + 4].copy_from_slice(&word.to_le_bytes());
        }
        out
    }

    pub fn decode(bytes: &[u8; HEADER_LEN]) -> Result<Self> {
        let word = |i: usize| u32::from_le_bytes(bytes[i * 4..i * 4 + 4].try_into().unwrap());
        let raw_command = word(0);
        if word(5) != raw_command ^ 0xffff_ffff {
            return Err(Error::BadMagic { command: raw_command });
        }
        Ok(Self {
            command: Command::try_from(raw_command)?,
            arg0: word(1),
            arg1: word(2),
            data_length: word(3),
            data_check: word(4),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub command: Command,
    pub arg0: u32,
    pub arg1: u32,
    pub payload: Vec<u8>,
}

pub fn checksum(payload: &[u8]) -> u32 {
    payload.iter().map(|&b| u32::from(b)).fold(0, u32::wrapping_add)
}

impl Message {
    pub fn new(command: Command, arg0: u32, arg1: u32, payload: impl Into<Vec<u8>>) -> Self {
        Self { command, arg0, arg1, payload: payload.into() }
    }

    /// The header for this message. The checksum is always filled in; it costs
    /// nothing and keeps us compatible with peers that still check it.
    pub fn header(&self) -> Header {
        Header {
            command: self.command,
            arg0: self.arg0,
            arg1: self.arg1,
            data_length: self.payload.len() as u32,
            data_check: checksum(&self.payload),
        }
    }

    pub async fn write_to<W: AsyncWrite + Unpin>(&self, w: &mut W) -> Result<()> {
        w.write_all(&self.header().encode()).await?;
        if !self.payload.is_empty() {
            w.write_all(&self.payload).await?;
        }
        Ok(())
    }

    /// Reads one message. `max_payload` is the negotiated limit;
    /// `verify_checksum` is true only while talking to a pre-skip-checksum peer.
    pub async fn read_from<R: AsyncRead + Unpin>(
        r: &mut R,
        max_payload: usize,
        verify_checksum: bool,
    ) -> Result<Self> {
        let mut raw = [0u8; HEADER_LEN];
        r.read_exact(&mut raw).await?;
        let header = Header::decode(&raw)?;
        let len = header.data_length as usize;
        if len > max_payload {
            return Err(Error::PayloadTooLarge { len, max: max_payload });
        }
        let mut payload = vec![0u8; len];
        r.read_exact(&mut payload).await?;
        if verify_checksum {
            let actual = checksum(&payload);
            if actual != header.data_check {
                return Err(Error::Checksum { expected: header.data_check, actual });
            }
        }
        Ok(Self { command: header.command, arg0: header.arg0, arg1: header.arg1, payload })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_values_spell_their_names() {
        // The command words are ASCII names read as little-endian u32s.
        for (cmd, name) in [
            (Command::Cnxn, b"CNXN"),
            (Command::Auth, b"AUTH"),
            (Command::Open, b"OPEN"),
            (Command::Okay, b"OKAY"),
            (Command::Clse, b"CLSE"),
            (Command::Wrte, b"WRTE"),
            (Command::Stls, b"STLS"),
            (Command::Sync, b"SYNC"),
        ] {
            assert_eq!(cmd as u32, u32::from_le_bytes(*name), "{cmd:?}");
        }
    }

    #[test]
    fn header_round_trip() {
        let h = Header { command: Command::Wrte, arg0: 7, arg1: 9, data_length: 3, data_check: 6 };
        assert_eq!(Header::decode(&h.encode()).unwrap(), h);
    }

    #[test]
    fn header_rejects_bad_magic() {
        let mut raw = Message::new(Command::Okay, 1, 2, vec![]).header().encode();
        raw[20] ^= 1;
        assert!(matches!(Header::decode(&raw), Err(Error::BadMagic { .. })));
    }

    #[test]
    fn header_rejects_unknown_command() {
        let mut raw = [0u8; HEADER_LEN];
        raw[..4].copy_from_slice(b"NOPE");
        raw[20..].copy_from_slice(&(u32::from_le_bytes(*b"NOPE") ^ 0xffff_ffff).to_le_bytes());
        assert!(matches!(Header::decode(&raw), Err(Error::UnknownCommand(_))));
    }

    #[test]
    fn cnxn_matches_known_bytes() {
        // First 24 bytes of a real host CNXN: version 0x01000001, maxdata 1 MiB.
        let msg = Message::new(Command::Cnxn, VERSION_SKIP_CHECKSUM, MAX_PAYLOAD as u32, b"host::\0".to_vec());
        let raw = msg.header().encode();
        assert_eq!(&raw[..4], b"CNXN");
        assert_eq!(&raw[4..8], &[0x01, 0x00, 0x00, 0x01]);
        assert_eq!(&raw[8..12], &[0x00, 0x00, 0x10, 0x00]);
        assert_eq!(u32::from_le_bytes(raw[12..16].try_into().unwrap()), 7);
        assert_eq!(u32::from_le_bytes(raw[16..20].try_into().unwrap()), checksum(b"host::\0"));
    }

    #[test]
    fn checksum_is_a_wrapping_byte_sum() {
        assert_eq!(checksum(&[]), 0);
        assert_eq!(checksum(&[1, 2, 3]), 6);
        assert_eq!(checksum(&[0xff; 4]), 0x3fc);
    }

    #[tokio::test]
    async fn message_round_trip_over_a_stream() {
        let (mut a, mut b) = tokio::io::duplex(64);
        let sent = Message::new(Command::Wrte, 3, 4, b"hello".to_vec());
        sent.write_to(&mut a).await.unwrap();
        let got = Message::read_from(&mut b, MAX_PAYLOAD, true).await.unwrap();
        assert_eq!(got, sent);
    }

    #[tokio::test]
    async fn read_rejects_oversized_payload() {
        let (mut a, mut b) = tokio::io::duplex(64);
        Message::new(Command::Wrte, 0, 0, vec![0; 10]).write_to(&mut a).await.unwrap();
        let err = Message::read_from(&mut b, 8, false).await.unwrap_err();
        assert!(matches!(err, Error::PayloadTooLarge { len: 10, max: 8 }));
    }

    #[tokio::test]
    async fn read_checks_checksum_only_when_asked() {
        let msg = Message::new(Command::Wrte, 0, 0, b"abc".to_vec());
        let mut raw = msg.header().encode().to_vec();
        raw[16] ^= 0xff; // corrupt data_check
        raw.extend_from_slice(b"abc");

        let got = Message::read_from(&mut raw.as_slice(), MAX_PAYLOAD, false).await.unwrap();
        assert_eq!(got.payload, b"abc");
        let err = Message::read_from(&mut raw.as_slice(), MAX_PAYLOAD, true).await.unwrap_err();
        assert!(matches!(err, Error::Checksum { .. }));
    }
}
