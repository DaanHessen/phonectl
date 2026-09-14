//! ADB wireless-debugging pairing.
//!
//! Ported from AOSP `adb/pairing_connection/` and `adb/pairing_auth/`:
//!
//! 1. TLS 1.3 with both sides presenting (self-signed) certificates.
//! 2. Export 64 bytes of keying material with the label `"adb-label\0"` and
//!    append it to the pairing code. Binding the password to the TLS session
//!    stops anyone from stealing the connection.
//! 3. SPAKE2 (BoringSSL flavour, see [`crate::spake2`]) with the names
//!    `"adb pair client\0"` and `"adb pair server\0"`.
//! 4. HKDF-SHA256 the SPAKE2 key into an AES-128-GCM key and exchange
//!    `PeerInfo` structures encrypted under it. Ours carries the host public
//!    key, which is what the phone stores as trusted.
//!
//! Every packet is a 6-byte header: version, type, then a big-endian payload
//! length.

use std::sync::Arc;

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes128Gcm, Nonce};
use hkdf::Hkdf;
use sha2::Sha256;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::auth::HostKey;
use crate::error::{Error, Result};
use crate::spake2::{Role, Spake2};

const HEADER_LEN: usize = 6;
const VERSION: u8 = 1;
const TYPE_SPAKE2_MSG: u8 = 0;
const TYPE_PEER_INFO: u8 = 1;

const PEER_INFO_LEN: usize = 8192;
const MAX_PAYLOAD: usize = PEER_INFO_LEN * 2;

const EXPORTED_KEY_LEN: usize = 64;
/// `sizeof(kExportedKeyLabel)` in AOSP includes the NUL byte.
const EXPORTED_KEY_LABEL: &[u8] = b"adb-label\0";

const CLIENT_NAME: &[u8] = b"adb pair client\0";
const SERVER_NAME: &[u8] = b"adb pair server\0";

/// `sizeof(info) - 1` in AOSP, so the NUL is not included here.
const HKDF_INFO: &[u8] = b"adb pairing_auth aes-128-gcm key";

/// What the peers exchange once the password is proven.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerInfo {
    pub kind: PeerInfoKind,
    /// For `RsaPublicKey`, the `base64 comment\0` line adbd stores.
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerInfoKind {
    RsaPublicKey,
    DeviceGuid,
    Unknown(u8),
}

impl PeerInfoKind {
    fn to_byte(self) -> u8 {
        match self {
            Self::RsaPublicKey => 0,
            Self::DeviceGuid => 1,
            Self::Unknown(other) => other,
        }
    }

    fn from_byte(byte: u8) -> Self {
        match byte {
            0 => Self::RsaPublicKey,
            1 => Self::DeviceGuid,
            other => Self::Unknown(other),
        }
    }
}

impl PeerInfo {
    /// Our side: the host public key, as adbd expects to store it.
    pub fn host_key(key: &HostKey, comment: &str) -> Self {
        Self { kind: PeerInfoKind::RsaPublicKey, data: key.public_key_payload(comment) }
    }

    /// The device GUID or key text, with padding removed.
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.data).trim_end_matches('\0').trim_end().to_string()
    }

    fn encode(&self) -> Result<[u8; PEER_INFO_LEN]> {
        if self.data.len() > PEER_INFO_LEN - 1 {
            return Err(Error::Protocol("peer info payload is too large".into()));
        }
        let mut out = [0u8; PEER_INFO_LEN];
        out[0] = self.kind.to_byte();
        out[1..1 + self.data.len()].copy_from_slice(&self.data);
        Ok(out)
    }

    fn decode(bytes: &[u8]) -> Result<Self> {
        if bytes.len() != PEER_INFO_LEN {
            return Err(Error::Protocol(format!(
                "peer info is {} bytes, expected {PEER_INFO_LEN}",
                bytes.len()
            )));
        }
        // The payload is a NUL-terminated string in a fixed-size buffer. Keep
        // the terminator, drop the padding after it.
        let body = &bytes[1..];
        let end = body.iter().position(|&b| b == 0).map_or(body.len(), |i| i + 1);
        Ok(Self { kind: PeerInfoKind::from_byte(bytes[0]), data: body[..end].to_vec() })
    }
}

/// AES-128-GCM with the counter nonces AOSP uses: the little-endian sequence
/// number in the first 8 bytes, zeros after, counted separately per direction.
struct Cipher {
    cipher: Aes128Gcm,
    encrypt_sequence: u64,
    decrypt_sequence: u64,
}

impl Cipher {
    fn new(key_material: &[u8]) -> Result<Self> {
        let hkdf = Hkdf::<Sha256>::new(None, key_material);
        let mut key = [0u8; 16];
        hkdf.expand(HKDF_INFO, &mut key).map_err(|e| Error::Protocol(format!("hkdf: {e}")))?;
        Ok(Self {
            cipher: Aes128Gcm::new_from_slice(&key).map_err(|e| Error::Protocol(e.to_string()))?,
            encrypt_sequence: 0,
            decrypt_sequence: 0,
        })
    }

    fn nonce(sequence: u64) -> [u8; 12] {
        let mut nonce = [0u8; 12];
        nonce[..8].copy_from_slice(&sequence.to_le_bytes());
        nonce
    }

    fn encrypt(&mut self, plaintext: &[u8]) -> Result<Vec<u8>> {
        let nonce = Self::nonce(self.encrypt_sequence);
        let out = self
            .cipher
            .encrypt(Nonce::from_slice(&nonce), Payload { msg: plaintext, aad: &[] })
            .map_err(|_| Error::Protocol("pairing encryption failed".into()))?;
        self.encrypt_sequence += 1;
        Ok(out)
    }

    fn decrypt(&mut self, ciphertext: &[u8]) -> Result<Vec<u8>> {
        let nonce = Self::nonce(self.decrypt_sequence);
        let out = self
            .cipher
            .decrypt(Nonce::from_slice(&nonce), Payload { msg: ciphertext, aad: &[] })
            .map_err(|_| Error::PairingRejected)?;
        self.decrypt_sequence += 1;
        Ok(out)
    }
}

async fn write_packet<S>(stream: &mut S, kind: u8, payload: &[u8]) -> Result<()>
where
    S: AsyncWrite + Unpin,
{
    let mut header = [0u8; HEADER_LEN];
    header[0] = VERSION;
    header[1] = kind;
    header[2..].copy_from_slice(&(payload.len() as u32).to_be_bytes());
    stream.write_all(&header).await?;
    stream.write_all(payload).await?;
    stream.flush().await?;
    Ok(())
}

async fn read_packet<S>(stream: &mut S, expected: u8) -> Result<Vec<u8>>
where
    S: AsyncRead + Unpin,
{
    let mut header = [0u8; HEADER_LEN];
    stream.read_exact(&mut header).await?;
    if header[0] != VERSION {
        return Err(Error::Protocol(format!("unsupported pairing packet version {}", header[0])));
    }
    if header[1] != expected {
        return Err(Error::Protocol(format!(
            "expected pairing packet type {expected}, got {}",
            header[1]
        )));
    }
    let length = u32::from_be_bytes(header[2..].try_into().unwrap()) as usize;
    if length == 0 || length > MAX_PAYLOAD {
        return Err(Error::Protocol(format!("pairing payload length {length} is out of range")));
    }
    let mut payload = vec![0u8; length];
    stream.read_exact(&mut payload).await?;
    Ok(payload)
}

/// Runs the pairing exchange over an already established TLS stream, given the
/// keying material exported from it.
///
/// Split out from [`pair`] so it can be tested without a phone.
pub async fn exchange<S>(
    stream: &mut S,
    role: Role,
    code: &str,
    exported_key: &[u8],
    ours: &PeerInfo,
) -> Result<PeerInfo>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let mut password = code.as_bytes().to_vec();
    password.extend_from_slice(exported_key);

    let (my_name, their_name) = match role {
        Role::Alice => (CLIENT_NAME, SERVER_NAME),
        Role::Bob => (SERVER_NAME, CLIENT_NAME),
    };
    let spake = Spake2::start(role, my_name, their_name, &password);

    write_packet(stream, TYPE_SPAKE2_MSG, spake.message()).await?;
    let their_msg = read_packet(stream, TYPE_SPAKE2_MSG).await?;
    let key_material = spake.finish(&their_msg)?;

    let mut cipher = Cipher::new(&key_material)?;
    let encrypted = cipher.encrypt(&ours.encode()?)?;
    write_packet(stream, TYPE_PEER_INFO, &encrypted).await?;

    let their_encrypted = read_packet(stream, TYPE_PEER_INFO).await?;
    PeerInfo::decode(&cipher.decrypt(&their_encrypted)?)
}

/// Pairs with a phone showing a pairing code.
///
/// `address` is the `_adb-tls-pairing._tcp` endpoint the phone advertises
/// while its "Pair device with pairing code" dialog is open. On success the
/// phone has stored our public key and will accept TLS connections from us.
pub async fn pair(
    address: std::net::SocketAddr,
    code: &str,
    key: &HostKey,
    comment: &str,
) -> Result<PeerInfo> {
    let socket = tokio::net::TcpStream::connect(address).await?;
    let config = crate::tls::client_config(key)?;
    let mut tls = crate::tls::upgrade(socket, Arc::clone(&config)).await?;

    let mut exported = [0u8; EXPORTED_KEY_LEN];
    export_keying_material(&tls, &mut exported)?;

    exchange(&mut tls, Role::Alice, code, &exported, &PeerInfo::host_key(key, comment)).await
}

fn export_keying_material<S>(
    tls: &tokio_rustls::client::TlsStream<S>,
    out: &mut [u8; EXPORTED_KEY_LEN],
) -> Result<()> {
    tls.get_ref()
        .1
        .export_keying_material(&mut out[..], EXPORTED_KEY_LABEL, None)
        .map_err(|e| Error::Tls(format!("cannot export keying material: {e}")))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> &'static HostKey {
        use std::sync::OnceLock;
        static KEY: OnceLock<HostKey> = OnceLock::new();
        KEY.get_or_init(|| HostKey::generate().unwrap())
    }

    #[test]
    fn peer_info_round_trip() {
        let info = PeerInfo::host_key(key(), "daan@omarchy");
        let decoded = PeerInfo::decode(&info.encode().unwrap()).unwrap();
        assert_eq!(decoded.kind, PeerInfoKind::RsaPublicKey);
        assert_eq!(decoded.data, info.data);
        assert!(decoded.text().ends_with("daan@omarchy"));
    }

    #[test]
    fn peer_info_rejects_oversized_and_short_payloads() {
        let too_big = PeerInfo { kind: PeerInfoKind::DeviceGuid, data: vec![7; PEER_INFO_LEN] };
        assert!(too_big.encode().is_err());
        assert!(PeerInfo::decode(&[0u8; 16]).is_err());
    }

    #[test]
    fn cipher_uses_counter_nonces_per_direction() {
        assert_eq!(Cipher::nonce(0), [0u8; 12]);
        let nonce = Cipher::nonce(1);
        assert_eq!(nonce[0], 1);
        assert_eq!(&nonce[1..], &[0u8; 11]);

        let mut a = Cipher::new(&[4u8; 64]).unwrap();
        let mut b = Cipher::new(&[4u8; 64]).unwrap();
        for message in [b"first".as_slice(), b"second", b"third"] {
            let sealed = a.encrypt(message).unwrap();
            assert_eq!(b.decrypt(&sealed).unwrap(), message);
        }
        assert_eq!(a.encrypt_sequence, 3);
        assert_eq!(b.decrypt_sequence, 3);
    }

    #[test]
    fn cipher_rejects_a_wrong_key() {
        let sealed = Cipher::new(&[1u8; 64]).unwrap().encrypt(b"hello").unwrap();
        let err = Cipher::new(&[2u8; 64]).unwrap().decrypt(&sealed).unwrap_err();
        assert!(matches!(err, Error::PairingRejected));
    }

    #[tokio::test]
    async fn packet_framing_round_trip() {
        let (mut ours, mut theirs) = tokio::io::duplex(256);
        write_packet(&mut ours, TYPE_SPAKE2_MSG, b"hello").await.unwrap();
        assert_eq!(read_packet(&mut theirs, TYPE_SPAKE2_MSG).await.unwrap(), b"hello");
    }

    #[tokio::test]
    async fn packet_reader_rejects_the_wrong_type() {
        let (mut ours, mut theirs) = tokio::io::duplex(256);
        write_packet(&mut ours, TYPE_PEER_INFO, b"x").await.unwrap();
        assert!(read_packet(&mut theirs, TYPE_SPAKE2_MSG).await.is_err());
    }

    #[tokio::test]
    async fn client_and_server_exchange_peer_info() {
        let (mut client_side, mut server_side) = tokio::io::duplex(64 * 1024);
        let exported = [9u8; EXPORTED_KEY_LEN];

        let server_info = PeerInfo { kind: PeerInfoKind::DeviceGuid, data: b"adb-0123456789ABCDE-abc\0".to_vec() };
        let expected_server = server_info.clone();
        let server = tokio::spawn(async move {
            exchange(&mut server_side, Role::Bob, "314159", &exported, &server_info).await
        });

        let client_info = PeerInfo::host_key(key(), "daan@omarchy");
        let got_server = exchange(&mut client_side, Role::Alice, "314159", &exported, &client_info)
            .await
            .unwrap();
        let got_client = server.await.unwrap().unwrap();

        assert_eq!(got_server, expected_server);
        assert_eq!(got_client, client_info);
    }

    #[tokio::test]
    async fn a_wrong_code_fails_to_decrypt() {
        let (mut client_side, mut server_side) = tokio::io::duplex(64 * 1024);
        let exported = [3u8; EXPORTED_KEY_LEN];
        let server_info = PeerInfo { kind: PeerInfoKind::DeviceGuid, data: b"phone\0".to_vec() };

        let server = tokio::spawn(async move {
            exchange(&mut server_side, Role::Bob, "000000", &exported, &server_info).await
        });

        let client = exchange(
            &mut client_side,
            Role::Alice,
            "999999",
            &exported,
            &PeerInfo::host_key(key(), "daan@omarchy"),
        )
        .await;

        assert!(matches!(client.unwrap_err(), Error::PairingRejected));
        assert!(server.await.unwrap().is_err());
    }

    #[tokio::test]
    async fn mismatched_keying_material_fails() {
        // Someone who joins the TLS session late has different exported keys.
        let (mut client_side, mut server_side) = tokio::io::duplex(64 * 1024);
        let server_info = PeerInfo { kind: PeerInfoKind::DeviceGuid, data: b"phone\0".to_vec() };

        let server = tokio::spawn(async move {
            exchange(&mut server_side, Role::Bob, "123456", &[1u8; EXPORTED_KEY_LEN], &server_info).await
        });

        let client = exchange(
            &mut client_side,
            Role::Alice,
            "123456",
            &[2u8; EXPORTED_KEY_LEN],
            &PeerInfo::host_key(key(), "daan@omarchy"),
        )
        .await;

        assert!(client.is_err());
        assert!(server.await.unwrap().is_err());
    }
}
