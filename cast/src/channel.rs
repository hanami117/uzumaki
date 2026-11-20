//! One CASTV2 connection: a thread owns the stream (TLS to the device, or a plain stream in tests),
//! writes what [`Channel::send`] queues, cuts what arrives into messages and hands them to the
//! caller. It answers the device's PINGs itself.
//!
//! The device's certificate is self-signed (its identity would be checked by Google's device
//! authentication, which senders may skip, as VLC and pychromecast do), so it is accepted as it is;
//! the TLS handshake's signatures are still verified.

use std::io::{ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Duration;

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{CryptoProvider, verify_tls12_signature, verify_tls13_signature};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, ClientConnection, DigitallySignedStruct, SignatureScheme, StreamOwned};
use serde_json::json;

use crate::proto::{Deframer, Message, NS_HEARTBEAT};

/// How long a read waits before the thread looks at its queue again.
const POLL: Duration = Duration::from_millis(30);

pub enum Incoming {
    Msg(Message),
    /// The connection ended (the device closed it, a read or write failed, or a bad frame).
    Closed(String),
}

pub struct Channel {
    tx: mpsc::Sender<Vec<u8>>,
    quit: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

#[derive(Debug)]
struct AnyCert(Arc<CryptoProvider>);

impl ServerCertVerifier for AnyCert {
    fn verify_server_cert(&self, _: &CertificateDer<'_>, _: &[CertificateDer<'_>], _: &ServerName<'_>, _: &[u8], _: UnixTime) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(&self, message: &[u8], cert: &CertificateDer<'_>, dss: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(message, cert, dss, &self.0.signature_verification_algorithms)
    }
    fn verify_tls13_signature(&self, message: &[u8], cert: &CertificateDer<'_>, dss: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(message, cert, dss, &self.0.signature_verification_algorithms)
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

fn tls_config() -> Result<Arc<ClientConfig>, String> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let cfg = ClientConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(AnyCert(provider)))
        .with_no_client_auth();
    Ok(Arc::new(cfg))
}

impl Channel {
    /// TLS to the device. Waits up to `timeout` for the TCP connection and the handshake.
    pub fn open_tls(addr: SocketAddr, timeout: Duration, on: impl Fn(Incoming) + Send + 'static) -> Result<Channel, String> {
        let mut tcp = TcpStream::connect_timeout(&addr, timeout).map_err(|e| format!("connect {addr}: {e}"))?;
        let _ = tcp.set_nodelay(true);
        tcp.set_read_timeout(Some(timeout)).map_err(|e| e.to_string())?;
        tcp.set_write_timeout(Some(timeout)).map_err(|e| e.to_string())?;
        let name = ServerName::from(addr.ip());
        let mut conn = ClientConnection::new(tls_config()?, name).map_err(|e| format!("TLS: {e}"))?;
        while conn.is_handshaking() {
            conn.complete_io(&mut tcp).map_err(|e| format!("TLS handshake with {addr}: {e}"))?;
        }
        tcp.set_read_timeout(Some(POLL)).map_err(|e| e.to_string())?;
        Ok(Self::spawn(StreamOwned::new(conn, tcp), on))
    }

    /// Runs the connection on `stream`, which must time out its reads after a short while (so the
    /// thread can write in between).
    pub fn spawn<S: Read + Write + Send + 'static>(mut stream: S, on: impl Fn(Incoming) + Send + 'static) -> Channel {
        let (tx, rx) = mpsc::channel::<Vec<u8>>();
        let quit = Arc::new(AtomicBool::new(false));
        let q = quit.clone();
        let thread = std::thread::Builder::new()
            .name("cast-channel".into())
            .spawn(move || {
                let mut de = Deframer::default();
                let mut buf = vec![0u8; 16384];
                let why = 'run: loop {
                    // What was queued goes out first, even when ending (a goodbye STOP/CLOSE).
                    loop {
                        match rx.try_recv() {
                            Ok(bytes) => {
                                if let Err(e) = stream.write_all(&bytes).and_then(|()| stream.flush()) {
                                    break 'run Some(format!("write: {e}"));
                                }
                            }
                            Err(mpsc::TryRecvError::Empty) => break,
                            Err(mpsc::TryRecvError::Disconnected) => break 'run None,
                        }
                    }
                    if q.load(Ordering::SeqCst) {
                        break 'run None;
                    }
                    match stream.read(&mut buf) {
                        Ok(0) => break 'run Some("closed by the device".into()),
                        Ok(n) => de.push(&buf[..n]),
                        Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut | ErrorKind::Interrupted) => continue,
                        Err(e) => break 'run Some(format!("read: {e}")),
                    }
                    while let Some(m) = de.take() {
                        match m {
                            Ok(m) if m.namespace == NS_HEARTBEAT && m.kind() == "PING" => {
                                let pong = Message::new(&m.destination, &m.source, NS_HEARTBEAT, &json!({"type": "PONG"}));
                                if let Err(e) = stream.write_all(&pong.frame()).and_then(|()| stream.flush()) {
                                    break 'run Some(format!("write: {e}"));
                                }
                            }
                            Ok(m) => on(Incoming::Msg(m)),
                            Err(e) => break 'run Some(format!("bad frame: {e}")),
                        }
                    }
                };
                if let Some(why) = why {
                    on(Incoming::Closed(why));
                }
            })
            .expect("spawn the cast channel thread");
        Channel { tx, quit, thread: Some(thread) }
    }

    /// Queues a message. False once the connection is gone.
    pub fn send(&self, m: &Message) -> bool {
        self.tx.send(m.frame()).is_ok()
    }
}

impl Drop for Channel {
    fn drop(&mut self) {
        self.quit.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}
