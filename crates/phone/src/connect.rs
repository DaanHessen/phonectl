//! Getting from "a phone is somewhere on the network" to a live ADB
//! connection, including the TLS upgrade wireless debugging requires.

use std::time::Duration;

use adb::discovery::{Discovered, Service};
use adb::transport::{Banner, Negotiated};
use adb::{Connection, Error, HostKey};

/// What a successful connection gives us.
pub struct Connected {
    pub connection: Connection,
    pub banner: Banner,
    pub discovered: Discovered,
}

/// Connects to an already paired phone over wireless debugging.
///
/// Wireless debugging always answers `CNXN` with `STLS`, so the socket is
/// upgraded and the handshake repeated on the TLS stream. If the phone does
/// not recognise our key it closes the connection during the TLS handshake;
/// that surfaces as [`Error::Tls`], which the caller should report as "pair
/// again".
pub async fn connect(discovered: Discovered, key: &HostKey, comment: &str) -> Result<Connected, Error> {
    let socket = tokio::net::TcpStream::connect(discovered.address).await?;
    socket.set_nodelay(true)?;

    let plain = match adb::negotiate(socket, key, comment).await? {
        Negotiated::NeedsTls { stream } => stream,
        Negotiated::Connected { stream, banner, max_payload, verify_checksum } => {
            // Only legacy `adb tcpip` connections get here. We never enable
            // that mode, but a phone left in it still works.
            let connection = Connection::start(stream, max_payload, verify_checksum);
            return Ok(Connected { connection, banner, discovered });
        }
        Negotiated::Unauthorized { .. } => return Err(Error::Unauthorized),
    };

    let tls = adb::tls::upgrade(plain, adb::tls::client_config(key)?).await?;
    match adb::negotiate(tls, key, comment).await? {
        Negotiated::Connected { stream, banner, max_payload, verify_checksum } => {
            let connection = Connection::start(stream, max_payload, verify_checksum);
            Ok(Connected { connection, banner, discovered })
        }
        Negotiated::Unauthorized { .. } => Err(Error::Unauthorized),
        Negotiated::NeedsTls { .. } => {
            Err(Error::Protocol("phone asked for TLS twice".into()))
        }
    }
}

/// Finds a paired phone and connects to it.
pub async fn discover_and_connect(
    instance: Option<&str>,
    key: &HostKey,
    comment: &str,
    timeout: Duration,
) -> Result<Connected, Error> {
    let discovered = adb::discovery::find(Service::Connect, instance, timeout).await?;
    connect(discovered, key, comment).await
}

/// Runs one shell command and returns its output.
///
/// Uses `shell,v2,raw:` when the phone supports it. In v2 the output is framed
/// (one byte id, four byte length, payload) so stdout and stderr stay apart;
/// we keep both and the exit code.
pub async fn shell(connection: &Connection, banner: &Banner, command: &str) -> Result<ShellOutput, Error> {
    if banner.has_feature("shell_v2") {
        let mut stream = connection.open(&format!("shell,v2,raw:{command}")).await?;
        let raw = stream.read_to_end().await;
        Ok(shell_v2_output(&raw))
    } else {
        let mut stream = connection.open(&format!("shell:{command}")).await?;
        let raw = stream.read_to_end().await;
        Ok(ShellOutput { stdout: raw, stderr: Vec::new(), exit_code: None })
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct ShellOutput {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub exit_code: Option<u8>,
}

impl ShellOutput {
    pub fn stdout_text(&self) -> String {
        String::from_utf8_lossy(&self.stdout).trim_end().to_string()
    }

    pub fn stderr_text(&self) -> String {
        String::from_utf8_lossy(&self.stderr).trim_end().to_string()
    }

    pub fn ok(&self) -> bool {
        self.exit_code.unwrap_or(0) == 0
    }
}

/// Packet ids in the shell v2 protocol (AOSP `adb/shell_protocol.h`).
const ID_STDOUT: u8 = 1;
const ID_STDERR: u8 = 2;
const ID_EXIT: u8 = 3;

fn shell_v2_output(raw: &[u8]) -> ShellOutput {
    let mut out = ShellOutput::default();
    let mut rest = raw;
    while rest.len() >= 5 {
        let id = rest[0];
        let length = u32::from_le_bytes(rest[1..5].try_into().unwrap()) as usize;
        let Some(body) = rest.get(5..5 + length) else { break };
        match id {
            ID_STDOUT => out.stdout.extend_from_slice(body),
            ID_STDERR => out.stderr.extend_from_slice(body),
            ID_EXIT => out.exit_code = body.first().copied(),
            _ => {}
        }
        rest = &rest[5 + length..];
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn packet(id: u8, body: &[u8]) -> Vec<u8> {
        let mut out = vec![id];
        out.extend_from_slice(&(body.len() as u32).to_le_bytes());
        out.extend_from_slice(body);
        out
    }

    #[test]
    fn shell_v2_splits_streams_and_exit_code() {
        let mut raw = packet(ID_STDOUT, b"hello ");
        raw.extend(packet(ID_STDOUT, b"world\n"));
        raw.extend(packet(ID_STDERR, b"careful\n"));
        raw.extend(packet(ID_EXIT, &[3]));

        let out = shell_v2_output(&raw);
        assert_eq!(out.stdout_text(), "hello world");
        assert_eq!(out.stderr_text(), "careful");
        assert_eq!(out.exit_code, Some(3));
        assert!(!out.ok());
    }

    #[test]
    fn shell_v2_ignores_truncated_and_unknown_packets() {
        let mut raw = packet(9, b"who knows");
        raw.extend(packet(ID_STDOUT, b"kept"));
        raw.extend_from_slice(&[ID_STDOUT, 0xff, 0xff, 0xff, 0xff, b'x']); // claims 4 GiB
        let out = shell_v2_output(&raw);
        assert_eq!(out.stdout_text(), "kept");
        assert_eq!(out.exit_code, None);
        assert!(out.ok(), "a missing exit code counts as success");
    }

    #[test]
    fn shell_v2_handles_empty_output() {
        let out = shell_v2_output(&packet(ID_EXIT, &[0]));
        assert_eq!(out.stdout_text(), "");
        assert_eq!(out.exit_code, Some(0));
    }
}
