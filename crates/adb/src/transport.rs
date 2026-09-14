//! Connection handshake and stream multiplexing.
//!
//! One driver task owns the socket. Everything else talks to it over a
//! channel, so there is exactly one writer and no lock ordering to get wrong.
//!
//! Flow control follows AOSP: after sending a `WRTE` on a stream we wait for
//! that stream's `OKAY` before sending the next one.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};

use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt, ReadHalf, WriteHalf};
use tokio::sync::{mpsc, oneshot};

use crate::auth::{AUTH_RSAPUBLICKEY, AUTH_SIGNATURE, AUTH_TOKEN, HostKey};
use crate::error::{Error, Result};
use crate::message::{Command, MAX_PAYLOAD, Message, VERSION_SKIP_CHECKSUM};

/// What the peer told us about itself in its `CNXN` banner.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Banner {
    /// "device", "recovery", "sideload", …
    pub kind: String,
    pub properties: BTreeMap<String, String>,
    pub features: BTreeSet<String>,
}

impl Banner {
    /// Parses `device::ro.product.name=pong;…;features=shell_v2,cmd`.
    pub fn parse(payload: &[u8]) -> Result<Self> {
        let text = std::str::from_utf8(payload.strip_suffix(b"\0").unwrap_or(payload))
            .map_err(|_| Error::Protocol("connection banner is not utf-8".into()))?;
        let (kind, rest) = text.split_once("::").unwrap_or((text, ""));

        let mut banner = Banner { kind: kind.to_string(), ..Default::default() };
        for field in rest.split(';').filter(|f| !f.is_empty()) {
            let Some((name, value)) = field.split_once('=') else { continue };
            if name == "features" {
                banner.features = value.split(',').filter(|f| !f.is_empty()).map(String::from).collect();
            } else {
                banner.properties.insert(name.to_string(), value.to_string());
            }
        }
        Ok(banner)
    }

    pub fn has_feature(&self, feature: &str) -> bool {
        self.features.contains(feature)
    }

    /// Best-effort human name for the device.
    pub fn model(&self) -> Option<&str> {
        self.properties.get("ro.product.model").map(String::as_str)
    }
}

/// Features we claim in our own banner. Only ones we actually implement.
pub const HOST_FEATURES: &[&str] = &["shell_v2", "cmd"];

pub fn host_banner() -> Vec<u8> {
    let mut banner = format!("host::features={}", HOST_FEATURES.join(",")).into_bytes();
    banner.push(0);
    banner
}

/// How the handshake ended.
pub enum Negotiated<S> {
    /// Ready to open streams.
    Connected { stream: S, banner: Banner, max_payload: usize, verify_checksum: bool },
    /// The phone wants TLS (wireless debugging). Upgrade the stream, then run
    /// `negotiate` again on the TLS stream.
    NeedsTls { stream: S },
    /// The phone does not know our key. It is now showing the "Allow USB
    /// debugging?" dialog; retry after the user accepts.
    Unauthorized { stream: S },
}

/// Performs the `CNXN`/`AUTH` handshake. `comment` labels our key on the
/// phone's authorisation dialog, e.g. `daan@omarchy`.
pub async fn negotiate<S>(mut stream: S, key: &HostKey, comment: &str) -> Result<Negotiated<S>>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    Message::new(Command::Cnxn, VERSION_SKIP_CHECKSUM, MAX_PAYLOAD as u32, host_banner())
        .write_to(&mut stream)
        .await?;

    let mut offered_public_key = false;
    loop {
        let msg = Message::read_from(&mut stream, MAX_PAYLOAD, false).await?;
        match msg.command {
            Command::Cnxn => {
                let banner = Banner::parse(&msg.payload)?;
                let max_payload = (msg.arg1 as usize).clamp(1, MAX_PAYLOAD);
                return Ok(Negotiated::Connected {
                    stream,
                    banner,
                    max_payload,
                    verify_checksum: msg.arg0 < VERSION_SKIP_CHECKSUM,
                });
            }
            Command::Stls => {
                Message::new(Command::Stls, crate::message::STLS_VERSION, 0, Vec::new())
                    .write_to(&mut stream)
                    .await?;
                return Ok(Negotiated::NeedsTls { stream });
            }
            Command::Auth if msg.arg0 == AUTH_TOKEN => {
                if offered_public_key {
                    // The phone re-challenged after seeing our public key: it is
                    // waiting for the user to accept it.
                    return Ok(Negotiated::Unauthorized { stream });
                }
                let signature = key.sign_token(&msg.payload)?;
                Message::new(Command::Auth, AUTH_SIGNATURE, 0, signature).write_to(&mut stream).await?;

                let next = Message::read_from(&mut stream, MAX_PAYLOAD, false).await?;
                match next.command {
                    Command::Cnxn => {
                        let banner = Banner::parse(&next.payload)?;
                        let max_payload = (next.arg1 as usize).clamp(1, MAX_PAYLOAD);
                        return Ok(Negotiated::Connected {
                            stream,
                            banner,
                            max_payload,
                            verify_checksum: next.arg0 < VERSION_SKIP_CHECKSUM,
                        });
                    }
                    Command::Auth if next.arg0 == AUTH_TOKEN => {
                        // Signature rejected: offer the public key instead.
                        Message::new(Command::Auth, AUTH_RSAPUBLICKEY, 0, key.public_key_payload(comment))
                            .write_to(&mut stream)
                            .await?;
                        offered_public_key = true;
                    }
                    other => {
                        return Err(Error::Unexpected { got: other, expected: "CNXN or AUTH" });
                    }
                }
            }
            other => return Err(Error::Unexpected { got: other, expected: "CNXN, AUTH or STLS" }),
        }
    }
}

enum Request {
    Open { service: String, reply: oneshot::Sender<Result<Stream>> },
    Write { local_id: u32, data: Vec<u8>, reply: oneshot::Sender<Result<()>> },
    Close { local_id: u32 },
}

/// A live connection to one device. Cloning is cheap; all clones talk to the
/// same driver task.
#[derive(Debug, Clone)]
pub struct Connection {
    requests: mpsc::Sender<Request>,
}

/// One multiplexed stream, i.e. one ADB service.
#[derive(Debug)]
pub struct Stream {
    local_id: u32,
    requests: mpsc::Sender<Request>,
    incoming: mpsc::Receiver<Vec<u8>>,
    max_payload: usize,
}

impl Connection {
    /// Takes over a stream that has finished `negotiate` and spawns its driver.
    pub fn start<S>(stream: S, max_payload: usize, verify_checksum: bool) -> Self
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let (requests, rx) = mpsc::channel(32);
        let (reader, writer) = tokio::io::split(stream);
        tokio::spawn(drive(reader, writer, rx, max_payload, verify_checksum));
        Self { requests }
    }

    /// Opens an ADB service, e.g. `shell,v2,raw:getprop` or
    /// `localabstract:phonectl`.
    pub async fn open(&self, service: &str) -> Result<Stream> {
        let (reply, answer) = oneshot::channel();
        self.requests
            .send(Request::Open { service: service.to_string(), reply })
            .await
            .map_err(|_| Error::ConnectionClosed)?;
        answer.await.map_err(|_| Error::ConnectionClosed)?
    }
}

impl Stream {
    /// Sends `data`, waiting for the peer's OKAY for each chunk.
    pub async fn write(&self, data: &[u8]) -> Result<()> {
        for chunk in data.chunks(self.max_payload) {
            let (reply, answer) = oneshot::channel();
            self.requests
                .send(Request::Write { local_id: self.local_id, data: chunk.to_vec(), reply })
                .await
                .map_err(|_| Error::ConnectionClosed)?;
            answer.await.map_err(|_| Error::ConnectionClosed)??;
        }
        Ok(())
    }

    /// Next chunk from the phone, or `None` once the stream is closed.
    pub async fn read(&mut self) -> Option<Vec<u8>> {
        self.incoming.recv().await
    }

    /// Reads until the peer closes the stream.
    pub async fn read_to_end(&mut self) -> Vec<u8> {
        let mut out = Vec::new();
        while let Some(chunk) = self.read().await {
            out.extend_from_slice(&chunk);
        }
        out
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        let _ = self.requests.try_send(Request::Close { local_id: self.local_id });
    }
}

struct StreamState {
    remote_id: u32,
    incoming: mpsc::Sender<Vec<u8>>,
    /// Writes waiting for their OKAY; the front one is in flight.
    pending: VecDeque<(Vec<u8>, oneshot::Sender<Result<()>>)>,
    in_flight: bool,
}

async fn drive<S>(
    reader: ReadHalf<S>,
    mut writer: WriteHalf<S>,
    mut requests: mpsc::Receiver<Request>,
    max_payload: usize,
    verify_checksum: bool,
) where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let mut reader = reader;
    let mut streams: HashMap<u32, StreamState> = HashMap::new();
    let mut opening: HashMap<u32, (oneshot::Sender<Result<Stream>>, mpsc::Sender<Request>)> = HashMap::new();
    let mut next_id: u32 = 1;

    // A handle we can clone into the Streams we hand out.
    let (self_tx, mut self_rx) = mpsc::channel::<Request>(32);

    loop {
        let incoming = Message::read_from(&mut reader, max_payload, verify_checksum);
        tokio::select! {
            message = incoming => {
                let Ok(message) = message else { break };
                match message.command {
                    Command::Okay => {
                        let local_id = message.arg1;
                        if let Some((reply, requests)) = opening.remove(&local_id) {
                            let (tx, rx) = mpsc::channel(16);
                            streams.insert(local_id, StreamState {
                                remote_id: message.arg0,
                                incoming: tx,
                                pending: VecDeque::new(),
                                in_flight: false,
                            });
                            let _ = reply.send(Ok(Stream { local_id, requests, incoming: rx, max_payload }));
                        } else if let Some(state) = streams.get_mut(&local_id) {
                            state.in_flight = false;
                            if let Some((_, reply)) = state.pending.pop_front() {
                                let _ = reply.send(Ok(()));
                            }
                            if let Some((data, _)) = state.pending.front() {
                                let msg = Message::new(Command::Wrte, local_id, state.remote_id, data.clone());
                                if msg.write_to(&mut writer).await.is_err() { break }
                                state.in_flight = true;
                            }
                        }
                    }
                    Command::Wrte => {
                        let local_id = message.arg1;
                        if let Some(state) = streams.get(&local_id) {
                            let remote_id = state.remote_id;
                            let _ = state.incoming.send(message.payload).await;
                            let ack = Message::new(Command::Okay, local_id, remote_id, Vec::new());
                            if ack.write_to(&mut writer).await.is_err() { break }
                        }
                    }
                    Command::Clse => {
                        let local_id = message.arg1;
                        streams.remove(&local_id);
                        if let Some((reply, _)) = opening.remove(&local_id) {
                            let _ = reply.send(Err(Error::ServiceRefused));
                        }
                    }
                    _ => {}
                }
            }
            request = requests.recv() => {
                let Some(request) = request else { break };
                if handle_request(request, &mut writer, &mut streams, &mut opening, &mut next_id, &self_tx).await.is_err() {
                    break;
                }
            }
            request = self_rx.recv() => {
                let Some(request) = request else { continue };
                if handle_request(request, &mut writer, &mut streams, &mut opening, &mut next_id, &self_tx).await.is_err() {
                    break;
                }
            }
        }
    }

    let _ = writer.shutdown().await;
}

async fn handle_request<S>(
    request: Request,
    writer: &mut WriteHalf<S>,
    streams: &mut HashMap<u32, StreamState>,
    opening: &mut HashMap<u32, (oneshot::Sender<Result<Stream>>, mpsc::Sender<Request>)>,
    next_id: &mut u32,
    self_tx: &mpsc::Sender<Request>,
) -> Result<()>
where
    S: AsyncWrite + Unpin,
{
    match request {
        Request::Open { service, reply } => {
            let local_id = *next_id;
            *next_id += 1;
            let mut payload = service.into_bytes();
            payload.push(0);
            Message::new(Command::Open, local_id, 0, payload).write_to(writer).await?;
            opening.insert(local_id, (reply, self_tx.clone()));
        }
        Request::Write { local_id, data, reply } => {
            let Some(state) = streams.get_mut(&local_id) else {
                let _ = reply.send(Err(Error::ConnectionClosed));
                return Ok(());
            };
            let send_now = !state.in_flight;
            state.pending.push_back((data.clone(), reply));
            if send_now {
                Message::new(Command::Wrte, local_id, state.remote_id, data).write_to(writer).await?;
                state.in_flight = true;
            }
        }
        Request::Close { local_id } => {
            if let Some(state) = streams.remove(&local_id) {
                Message::new(Command::Clse, local_id, state.remote_id, Vec::new()).write_to(writer).await?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use tokio::io::DuplexStream;

    use super::*;
    use crate::message::MAX_PAYLOAD;

    fn key() -> &'static HostKey {
        use std::sync::OnceLock;
        static KEY: OnceLock<HostKey> = OnceLock::new();
        KEY.get_or_init(|| HostKey::generate().unwrap())
    }

    async fn read(stream: &mut DuplexStream) -> Message {
        Message::read_from(stream, MAX_PAYLOAD, false).await.unwrap()
    }

    #[test]
    fn banner_parsing() {
        let banner = Banner::parse(b"device::ro.product.name=pong;ro.product.model=A069P;features=shell_v2,cmd\0").unwrap();
        assert_eq!(banner.kind, "device");
        assert_eq!(banner.model(), Some("A069P"));
        assert!(banner.has_feature("shell_v2"));
        assert!(!banner.has_feature("abb"));
    }

    #[test]
    fn banner_survives_missing_fields() {
        let banner = Banner::parse(b"device::").unwrap();
        assert_eq!(banner.kind, "device");
        assert!(banner.features.is_empty());
        assert_eq!(banner.model(), None);
    }

    #[test]
    fn host_banner_is_nul_terminated() {
        let banner = host_banner();
        assert_eq!(banner.last(), Some(&0));
        assert!(String::from_utf8_lossy(&banner).starts_with("host::features="));
    }

    #[tokio::test]
    async fn handshake_signs_the_auth_token() {
        let (ours, mut theirs) = tokio::io::duplex(4096);
        let phone = tokio::spawn(async move {
            let cnxn = read(&mut theirs).await;
            assert_eq!(cnxn.command, Command::Cnxn);
            Message::new(Command::Auth, AUTH_TOKEN, 0, vec![9u8; 20]).write_to(&mut theirs).await.unwrap();

            let auth = read(&mut theirs).await;
            assert_eq!(auth.command, Command::Auth);
            assert_eq!(auth.arg0, AUTH_SIGNATURE);
            rsa::RsaPublicKey::from(key().private_key())
                .verify(rsa::Pkcs1v15Sign::new::<sha1::Sha1>(), &[9u8; 20], &auth.payload)
                .expect("signature must verify");

            Message::new(Command::Cnxn, VERSION_SKIP_CHECKSUM, 256 * 1024, b"device::ro.product.model=A069P;features=shell_v2\0".to_vec())
                .write_to(&mut theirs)
                .await
                .unwrap();
        });

        let result = negotiate(ours, key(), "test@host").await.unwrap();
        phone.await.unwrap();
        match result {
            Negotiated::Connected { banner, max_payload, verify_checksum, .. } => {
                assert_eq!(banner.model(), Some("A069P"));
                assert_eq!(max_payload, 256 * 1024);
                assert!(!verify_checksum);
            }
            _ => panic!("expected Connected"),
        }
    }

    #[tokio::test]
    async fn handshake_offers_public_key_when_signature_is_rejected() {
        let (ours, mut theirs) = tokio::io::duplex(4096);
        let phone = tokio::spawn(async move {
            read(&mut theirs).await; // CNXN
            Message::new(Command::Auth, AUTH_TOKEN, 0, vec![1u8; 20]).write_to(&mut theirs).await.unwrap();
            read(&mut theirs).await; // signature
            Message::new(Command::Auth, AUTH_TOKEN, 0, vec![2u8; 20]).write_to(&mut theirs).await.unwrap();
            let offer = read(&mut theirs).await;
            assert_eq!(offer.arg0, AUTH_RSAPUBLICKEY);
            assert!(offer.payload.ends_with(b"test@host\0"));
            // The user has not tapped "allow" yet: challenge again.
            Message::new(Command::Auth, AUTH_TOKEN, 0, vec![3u8; 20]).write_to(&mut theirs).await.unwrap();
        });

        let result = negotiate(ours, key(), "test@host").await.unwrap();
        phone.await.unwrap();
        assert!(matches!(result, Negotiated::Unauthorized { .. }));
    }

    #[tokio::test]
    async fn handshake_answers_stls_and_asks_for_tls() {
        let (ours, mut theirs) = tokio::io::duplex(4096);
        let phone = tokio::spawn(async move {
            read(&mut theirs).await; // CNXN
            Message::new(Command::Stls, crate::message::STLS_VERSION, 0, Vec::new()).write_to(&mut theirs).await.unwrap();
            let reply = read(&mut theirs).await;
            assert_eq!(reply.command, Command::Stls);
            assert_eq!(reply.arg0, crate::message::STLS_VERSION);
        });

        let result = negotiate(ours, key(), "test@host").await.unwrap();
        phone.await.unwrap();
        assert!(matches!(result, Negotiated::NeedsTls { .. }));
    }

    #[tokio::test]
    async fn open_stream_echoes_data_and_acks() {
        let (ours, mut theirs) = tokio::io::duplex(4096);
        let phone = tokio::spawn(async move {
            let open = read(&mut theirs).await;
            assert_eq!(open.command, Command::Open);
            assert_eq!(open.payload, b"shell,v2,raw:echo hi\0");
            let local_id = open.arg0;
            Message::new(Command::Okay, 77, local_id, Vec::new()).write_to(&mut theirs).await.unwrap();

            // Phone sends output, expects an OKAY back.
            Message::new(Command::Wrte, 77, local_id, b"hi\n".to_vec()).write_to(&mut theirs).await.unwrap();
            let ack = read(&mut theirs).await;
            assert_eq!(ack.command, Command::Okay);

            // Host sends input; we ack it.
            let wrte = read(&mut theirs).await;
            assert_eq!(wrte.command, Command::Wrte);
            assert_eq!(wrte.payload, b"ping");
            Message::new(Command::Okay, 77, local_id, Vec::new()).write_to(&mut theirs).await.unwrap();

            Message::new(Command::Clse, 77, local_id, Vec::new()).write_to(&mut theirs).await.unwrap();
        });

        let connection = Connection::start(ours, MAX_PAYLOAD, false);
        let mut stream = connection.open("shell,v2,raw:echo hi").await.unwrap();
        assert_eq!(stream.read().await.unwrap(), b"hi\n");
        stream.write(b"ping").await.unwrap();
        assert_eq!(stream.read().await, None, "CLSE ends the stream");
        phone.await.unwrap();
    }

    #[tokio::test]
    async fn refused_service_reports_an_error() {
        let (ours, mut theirs) = tokio::io::duplex(4096);
        tokio::spawn(async move {
            let open = read(&mut theirs).await;
            Message::new(Command::Clse, 0, open.arg0, Vec::new()).write_to(&mut theirs).await.unwrap();
        });

        let connection = Connection::start(ours, MAX_PAYLOAD, false);
        let err = connection.open("nope:").await.unwrap_err();
        assert!(matches!(err, Error::ServiceRefused), "{err}");
    }

    #[tokio::test]
    async fn writes_are_chunked_to_the_payload_limit() {
        let (ours, mut theirs) = tokio::io::duplex(8192);
        let phone = tokio::spawn(async move {
            let open = read(&mut theirs).await;
            let local_id = open.arg0;
            Message::new(Command::Okay, 5, local_id, Vec::new()).write_to(&mut theirs).await.unwrap();
            let mut sizes = Vec::new();
            for _ in 0..3 {
                let wrte = read(&mut theirs).await;
                sizes.push(wrte.payload.len());
                Message::new(Command::Okay, 5, local_id, Vec::new()).write_to(&mut theirs).await.unwrap();
            }
            sizes
        });

        let connection = Connection::start(ours, 4, false);
        let stream = connection.open("sync:").await.unwrap();
        stream.write(b"0123456789").await.unwrap();
        assert_eq!(phone.await.unwrap(), vec![4, 4, 2]);
    }
}
