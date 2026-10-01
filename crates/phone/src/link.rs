//! The link protocol between the laptop daemon and the phonectl Android app.
//!
//! Newline-delimited JSON over a byte stream, identical on every transport
//! (Tailscale TCP, Bluetooth RFCOMM). The phone always dials; the laptop
//! answers. The Kotlin side is `android/app/src/main/java/com/daanh/phonectl/
//! Protocol.kt`; the wire-shape tests below pin what it sends.
//!
//! ```text
//! phone  → {"type":"hello","protocol":1,"nonce":n1,"device":{…},"tailscale_ip":"100.…"}
//! laptop → {"type":"challenge","nonce":n2,"proof":hmac(key,"laptop",n1,n2)}
//! phone  → {"type":"auth","proof":hmac(key,"phone",n1,n2)}
//! laptop → {"type":"ready","name":"omarchy"}
//! ```
//!
//! After that: `event` (either way), `call`/`result` (either way), and
//! `ping`/`pong` (laptop asks, phone answers). The transports already encrypt
//! (WireGuard, Bluetooth link encryption); the HMAC handshake proves both ends
//! hold the pairing key.

use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const PROTOCOL_VERSION: u32 = 1;
pub const TCP_PORT: u16 = 47201;
pub const POKE_PORT: u16 = 47202;
pub const BT_UUID: &str = "8f0c6a1e-3b5d-4c8e-9a71-5d2c4e6b7a10";
pub const MAX_LINE: usize = 2 * 1024 * 1024;
pub const MAX_CLIP: usize = 1024 * 1024;
const POKE_MAGIC: &[u8] = b"phonectl-poke";

/// Everything except the handshake lines.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Message {
    Event {
        topic: String,
        #[serde(default)]
        data: serde_json::Value,
    },
    Call {
        id: u64,
        method: String,
        #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
        params: serde_json::Value,
    },
    Result {
        id: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        data: Option<serde_json::Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    Ping {
        id: u64,
    },
    Pong {
        #[serde(default)]
        id: Option<u64>,
    },
    /// Laptop → phone: about to suspend; do not retry until poked.
    Sleeping,
    /// Anything a newer peer might send.
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Hello {
    pub protocol: u32,
    pub nonce: String,
    pub device: DeviceInfo,
    #[serde(default)]
    pub tailscale_ip: Option<String>,
    #[serde(default)]
    pub app_version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct DeviceInfo {
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub manufacturer: String,
    #[serde(default)]
    pub device: String,
    #[serde(default)]
    pub android_version: String,
    #[serde(default)]
    pub sdk: u32,
    #[serde(default)]
    pub build: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
}

impl DeviceInfo {
    pub fn display_name(&self) -> String {
        self.name.clone().filter(|n| !n.is_empty()).unwrap_or_else(|| self.model.clone())
    }
}

/// A notification as the phone describes it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Notification {
    pub key: String,
    pub package: String,
    #[serde(default)]
    pub app: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub posted_at: Option<i64>,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub clearable: bool,
    #[serde(default)]
    pub can_open: bool,
    #[serde(default)]
    pub silent: bool,
    #[serde(default)]
    pub update: bool,
    #[serde(default)]
    pub actions: Vec<NotificationAction>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotificationAction {
    pub index: u32,
    pub title: String,
    #[serde(default)]
    pub reply: bool,
}

/// A clipboard change, from either side.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Clip {
    pub text: String,
    pub hash: String,
    /// Unix millis when the clip was made on its origin.
    pub at: i64,
}

impl Clip {
    pub fn new(text: String, at: i64) -> Self {
        let hash = clip_hash(&text);
        Clip { text, hash, at }
    }
}

/// Same as `Protocol.clipHash` on the phone: first 128 bits of SHA-256, hex.
pub fn clip_hash(text: &str) -> String {
    hex(&Sha256::digest(text.as_bytes()))[..32].to_string()
}

pub fn nonce() -> String {
    let mut bytes = [0u8; 16];
    rand::Rng::fill(&mut rand::thread_rng(), &mut bytes);
    hex(&bytes)
}

fn hmac(key: &[u8], data: &[u8]) -> [u8; 32] {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC takes any key length");
    mac.update(data);
    mac.finalize().into_bytes().into()
}

/// The handshake proof for `role` ("phone" or "laptop").
pub fn proof(key: &[u8], role: &str, phone_nonce: &str, laptop_nonce: &str) -> String {
    hex(&hmac(key, format!("phonectl-auth|{role}|{phone_nonce}|{laptop_nonce}").as_bytes()))
}

/// Constant-time proof check.
pub fn proof_matches(expected: &str, given: &str) -> bool {
    use subtle::ConstantTimeEq;
    expected.len() == given.len() && bool::from(expected.as_bytes().ct_eq(given.as_bytes()))
}

/// The UDP datagram that asks the phone to connect now.
pub fn poke_packet(key: &[u8], unix_millis: u64) -> Vec<u8> {
    let mut packet = POKE_MAGIC.to_vec();
    packet.extend_from_slice(&unix_millis.to_be_bytes());
    let mac = hmac(key, &packet);
    packet.extend_from_slice(&mac);
    packet
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Encodes one message as a protocol line, newline included.
pub fn line(message: &impl Serialize) -> serde_json::Result<String> {
    Ok(format!("{}\n", serde_json::to_string(message)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn clip_hash_matches_the_kotlin_side() {
        // printf 'hello' | sha256sum | cut -c1-32
        assert_eq!(clip_hash("hello"), "2cf24dba5fb0a30e26e83b2ac5b9e29e");
    }

    #[test]
    fn proofs_differ_by_role_and_nonce() {
        let key = [7u8; 32];
        let laptop = proof(&key, "laptop", "a", "b");
        assert_ne!(laptop, proof(&key, "phone", "a", "b"));
        assert_ne!(laptop, proof(&key, "laptop", "a", "c"));
        assert!(proof_matches(&laptop, &proof(&key, "laptop", "a", "b")));
        assert!(!proof_matches(&laptop, ""));
        assert!(!proof_matches(&laptop, &proof(&[8u8; 32], "laptop", "a", "b")));
    }

    #[test]
    fn known_proof_vector() {
        // Pinned so the Kotlin unit test can assert the same value.
        let key = [1u8; 32];
        assert_eq!(proof(&key, "phone", "00", "11"), "1a32a59da8240ccd7046452fe7432aa0181f562777dc027e98686f2269a94d55");
        assert_eq!(
            hex(&poke_packet(&[3u8; 32], 1_790_000_000_000)),
            "70686f6e6563746c2d706f6b65000001a0c4506c0080866bc5c74d4432c01ab8f26a0a3873cb8d640e6f6a6d6411ee235fd9ec2d64"
        );
    }

    #[test]
    fn poke_layout() {
        let packet = poke_packet(&[3u8; 32], 0x0102_0304_0506_0708);
        assert_eq!(packet.len(), 13 + 8 + 32);
        assert_eq!(&packet[..13], b"phonectl-poke");
        assert_eq!(&packet[13..21], &[1, 2, 3, 4, 5, 6, 7, 8]);
    }

    #[test]
    fn wire_shapes() {
        let ping = serde_json::to_value(Message::Ping { id: 3 }).unwrap();
        assert_eq!(ping, json!({"type": "ping", "id": 3}));
        let event = serde_json::to_value(Message::Event { topic: "clipboard".into(), data: json!({"text": "x"}) }).unwrap();
        assert_eq!(event, json!({"type": "event", "topic": "clipboard", "data": {"text": "x"}}));
        let call = serde_json::to_value(Message::Call { id: 1, method: "icon".into(), params: json!({"package": "p"}) }).unwrap();
        assert_eq!(call, json!({"type": "call", "id": 1, "method": "icon", "params": {"package": "p"}}));
        assert_eq!(serde_json::to_value(Message::Sleeping).unwrap(), json!({"type": "sleeping"}));
    }

    #[test]
    fn parses_what_the_phone_sends() {
        let pong: Message = serde_json::from_str(r#"{"type":"pong","id":4}"#).unwrap();
        assert_eq!(pong, Message::Pong { id: Some(4) });
        let result: Message = serde_json::from_str(r#"{"type":"result","id":2,"data":null}"#).unwrap();
        assert!(matches!(result, Message::Result { id: 2, error: None, .. }));
        let failed: Message = serde_json::from_str(r#"{"type":"result","id":2,"error":"gone"}"#).unwrap();
        assert!(matches!(failed, Message::Result { error: Some(_), .. }));
        let future: Message = serde_json::from_str(r#"{"type":"something_new","x":1}"#).unwrap();
        assert_eq!(future, Message::Unknown);

        let n: Notification = serde_json::from_str(
            r#"{"key":"0|com.whatsapp|1|null|10123","package":"com.whatsapp","app":"WhatsApp","title":"Bob",
                "text":"hi","posted_at":1,"category":"msg","clearable":true,"can_open":true,"silent":false,
                "actions":[{"index":0,"title":"Reply","reply":true},{"index":1,"title":"Mark as read","reply":false}],"update":false}"#,
        )
        .unwrap();
        assert_eq!(n.actions.len(), 2);
        assert!(n.actions[0].reply);

        let hello: Hello = serde_json::from_str(
            r#"{"type":"hello","protocol":1,"nonce":"ab","device":{"model":"A069P","manufacturer":"Nothing","device":"FroggerPro",
                "android_version":"16","sdk":36,"build":"B4.1","name":"Nothing Phone (4a) Pro"},"app_version":"0.1.0","tailscale_ip":"100.88.77.66"}"#,
        )
        .unwrap();
        assert_eq!(hello.device.display_name(), "Nothing Phone (4a) Pro");
        assert_eq!(hello.tailscale_ip.as_deref(), Some("100.88.77.66"));
    }
}
