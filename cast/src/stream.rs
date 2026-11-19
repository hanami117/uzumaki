//! The HTTP server the Cast device fetches the audio from. The Default Media Receiver plays a URL,
//! so the live PCM is served as an endless WAV (or FLAC) response.
//!
//! Who can listen (the stream is the user's own music, for the user's own device only):
//! - bound to the one local address the system routes to the device, never to every interface;
//! - the path holds a random 128-bit token, new for each `LOAD`; anything else is a 404, and a
//!   connection for an older token ends;
//! - only the allowed addresses (the device; for a Cast Group the Cast devices seen) are served,
//!   others get a 403 and a log line;
//! - at most [`MAX_CLIENTS`] at once; nothing is written to disk; the server ends with its session.

use std::io::{Read, Write};
use std::net::{IpAddr, SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use crate::flac;
use crate::pcm::{FRAME_BYTES, Hub, RATE};

pub const MAX_CLIENTS: usize = 2;
/// Chunks (~93 ms each from the bridge) that may wait for a connection before it loses some.
const DEPTH: usize = 64;
/// A device that stops reading for this long (paused) loses its connection.
const WRITE_TIMEOUT: Duration = Duration::from_secs(120);

fn log(msg: impl AsRef<str>) {
    eprintln!("cast: {}", msg.as_ref());
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Wav,
    Flac,
}

impl Format {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "wav" => Some(Format::Wav),
            "flac" => Some(Format::Flac),
            _ => None,
        }
    }
    pub fn ext(self) -> &'static str {
        match self {
            Format::Wav => "wav",
            Format::Flac => "flac",
        }
    }
    pub fn content_type(self) -> &'static str {
        match self {
            Format::Wav => "audio/wav",
            Format::Flac => "audio/flac",
        }
    }
}

/// A WAV header for a stream of unknown length (sizes 0xFFFFFFFF).
pub fn wav_header() -> Vec<u8> {
    let mut h = Vec::with_capacity(44);
    h.extend_from_slice(b"RIFF");
    h.extend_from_slice(&u32::MAX.to_le_bytes());
    h.extend_from_slice(b"WAVEfmt ");
    h.extend_from_slice(&16u32.to_le_bytes());
    h.extend_from_slice(&1u16.to_le_bytes()); // PCM
    h.extend_from_slice(&2u16.to_le_bytes());
    h.extend_from_slice(&(RATE as u32).to_le_bytes());
    h.extend_from_slice(&(RATE as u32 * 4).to_le_bytes());
    h.extend_from_slice(&4u16.to_le_bytes());
    h.extend_from_slice(&16u16.to_le_bytes());
    h.extend_from_slice(b"data");
    h.extend_from_slice(&u32::MAX.to_le_bytes());
    h
}

/// The local address the system uses to reach `device` (no packet is sent).
pub fn local_addr_for(device: IpAddr) -> Option<IpAddr> {
    let any: SocketAddr = if device.is_ipv4() { "0.0.0.0:0".parse().ok()? } else { "[::]:0".parse().ok()? };
    let s = UdpSocket::bind(any).ok()?;
    s.connect((device, 8009)).ok()?;
    Some(s.local_addr().ok()?.ip())
}

/// 128 random bits, hex.
pub fn token() -> String {
    use ring::rand::SecureRandom;
    let mut b = [0u8; 16];
    ring::rand::SystemRandom::new().fill(&mut b).expect("system random");
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn plain(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(ip, IpAddr::V4),
        v4 => v4,
    }
}

pub fn allowed(peer: IpAddr, allow: &[IpAddr]) -> bool {
    allow.iter().any(|a| plain(*a) == plain(peer))
}

/// Who may fetch the stream (the token in the path is needed as well).
#[derive(Clone, Debug, Default)]
pub struct Allow {
    /// Every address the device advertises.
    device: Vec<IpAddr>,
    /// The /64 prefixes of the device's global IPv6 addresses (served over IPv6 only): its
    /// temporary privacy addresses come and go inside them.
    nets64: Vec<u64>,
    /// For a Cast Group: the Cast devices seen on the network.
    others: Vec<IpAddr>,
}

fn prefix64(a: IpAddr) -> Option<u64> {
    match plain(a) {
        IpAddr::V6(v) if (v.segments()[0] & 0xffc0) != 0xfe80 => Some((u128::from(v) >> 64) as u64),
        _ => None,
    }
}

impl Allow {
    /// The device's addresses; `v6_nets`: also any address in their /64s.
    pub fn device(addrs: &[IpAddr], v6_nets: bool) -> Self {
        let mut nets64: Vec<u64> = if v6_nets { addrs.iter().filter_map(|a| prefix64(*a)).collect() } else { Vec::new() };
        nets64.dedup();
        Allow { device: addrs.to_vec(), nets64, others: Vec::new() }
    }

    pub fn add_others(&mut self, more: &[IpAddr]) {
        for ip in more {
            if !self.others.contains(ip) && !self.device.contains(ip) {
                self.others.push(*ip);
            }
        }
    }

    /// The rule that lets `peer` in, if any.
    pub fn rule(&self, peer: IpAddr) -> Option<&'static str> {
        if allowed(peer, &self.device) {
            return Some("an address the device advertises");
        }
        if prefix64(peer).is_some_and(|p| self.nets64.contains(&p)) {
            return Some("in the same /64 as the device");
        }
        if allowed(peer, &self.others) {
            return Some("a Cast device seen on the network (group)");
        }
        None
    }

    pub fn describe(&self) -> String {
        let mut parts: Vec<String> = self.device.iter().map(ToString::to_string).collect();
        parts.extend(self.nets64.iter().map(|p| {
            let s = std::net::Ipv6Addr::from(u128::from(*p) << 64);
            format!("{s}/64")
        }));
        parts.extend(self.others.iter().map(|o| format!("{o} (group)")));
        parts.join(", ")
    }
}

/// One fetch by the device.
pub struct Fetch {
    pub peer: IpAddr,
    pub started: Instant,
    /// PCM frames written to the device (the stream's own time: frames / 44100).
    pub frames: AtomicU64,
    pub token_gen: u64,
}

impl Fetch {
    pub fn served_secs(&self) -> f64 {
        self.frames.load(Ordering::Relaxed) as f64 / RATE as f64
    }
}

struct State {
    token: String,
    generation: u64,
    allow: Allow,
    newest: Option<Arc<Fetch>>,
}

pub struct Server {
    addr: SocketAddr,
    format: Format,
    state: Arc<Mutex<State>>,
    generation: Arc<AtomicU64>,
    quit: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
    /// Requests refused (wrong address, wrong path): for the log and the tests.
    pub refused: Arc<AtomicUsize>,
}

impl Server {
    /// Listens on `bind:port` (0: any free port).
    pub fn start(bind: IpAddr, port: u16, format: Format, hub: Arc<Hub>, allow: Allow) -> std::io::Result<Server> {
        let listener = TcpListener::bind((bind, port))?;
        listener.set_nonblocking(true)?;
        let addr = listener.local_addr()?;
        let state = Arc::new(Mutex::new(State { token: token(), generation: 1, allow, newest: None }));
        let generation = Arc::new(AtomicU64::new(1));
        let quit = Arc::new(AtomicBool::new(false));
        let refused = Arc::new(AtomicUsize::new(0));
        let active = Arc::new(AtomicUsize::new(0));
        let (st, g, q, r) = (state.clone(), generation.clone(), quit.clone(), refused.clone());
        let thread = std::thread::Builder::new().name("cast-http".into()).spawn(move || {
            while !q.load(Ordering::SeqCst) {
                let (conn, peer) = match listener.accept() {
                    Ok(c) => c,
                    Err(_) => {
                        std::thread::sleep(Duration::from_millis(25));
                        continue;
                    }
                };
                let (st, g, q, r, hub, active) = (st.clone(), g.clone(), q.clone(), r.clone(), hub.clone(), active.clone());
                let _ = std::thread::Builder::new().name("cast-http-conn".into()).spawn(move || {
                    serve(conn, peer, format, &st, &g, &q, &r, &hub, &active);
                });
            }
        })?;
        log(format!("stream server on http://{addr}/ ({})", format.ext()));
        Ok(Server { addr, format, state, generation, quit, thread: Some(thread), refused })
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// A new token (for a new `LOAD`): the old URL stops working and its connections end.
    pub fn rotate(&self) -> String {
        let mut s = self.state.lock().unwrap();
        s.token = token();
        s.generation += 1;
        self.generation.store(s.generation, Ordering::SeqCst);
        self.url_of(&s.token)
    }

    pub fn url(&self) -> String {
        self.url_of(&self.state.lock().unwrap().token)
    }

    fn url_of(&self, token: &str) -> String {
        let host = match self.addr.ip() {
            IpAddr::V6(v6) => format!("[{v6}]"),
            v4 => v4.to_string(),
        };
        format!("http://{host}:{}/cast/{token}/stream.{}", self.addr.port(), self.format.ext())
    }

    pub fn allow(&self, more: &[IpAddr]) {
        self.state.lock().unwrap().allow.add_others(more);
    }

    /// The device's newest fetch of the current URL.
    pub fn newest(&self) -> Option<Arc<Fetch>> {
        let s = self.state.lock().unwrap();
        s.newest.clone().filter(|f| f.token_gen == s.generation)
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.quit.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn respond(conn: &mut TcpStream, status: &str) {
    let _ = write!(conn, "HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
}

/// Reads the request head (up to 8 KB, 5 s).
fn read_head(conn: &mut TcpStream) -> Option<String> {
    let _ = conn.set_read_timeout(Some(Duration::from_secs(5)));
    let mut head = Vec::new();
    let mut buf = [0u8; 1024];
    while !head.windows(4).any(|w| w == b"\r\n\r\n") {
        let n = conn.read(&mut buf).ok()?;
        if n == 0 || head.len() > 8192 {
            return None;
        }
        head.extend_from_slice(&buf[..n]);
    }
    Some(String::from_utf8_lossy(&head).into_owned())
}

#[allow(clippy::too_many_arguments)]
fn serve(mut conn: TcpStream, peer: SocketAddr, format: Format, st: &Mutex<State>, generation: &AtomicU64, quit: &AtomicBool, refused: &AtomicUsize, hub: &Hub, active: &AtomicUsize) {
    let _ = conn.set_nonblocking(false);
    let ip = plain(peer.ip());
    let (token, my_gen, ok) = {
        let s = st.lock().unwrap();
        (s.token.clone(), s.generation, s.allow.rule(ip))
    };
    // The head is read first even when refusing: closing with it unread would reset the
    // connection and lose the answer.
    let Some(head) = read_head(&mut conn) else { return };
    let Some(rule) = ok else {
        refused.fetch_add(1, Ordering::SeqCst);
        log(format!("refused a request from {ip}: not the Cast device"));
        return respond(&mut conn, "403 Forbidden");
    };
    let mut lines = head.lines();
    let request = lines.next().unwrap_or("");
    let mut parts = request.split_whitespace();
    let (method, path) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
    let header = |name: &str| head.lines().skip(1).find_map(|l| l.split_once(':').filter(|(k, _)| k.trim().eq_ignore_ascii_case(name)).map(|(_, v)| v.trim().to_string()));
    let want = format!("/cast/{token}/stream.{}", format.ext());
    if path != want || !(method == "GET" || method == "HEAD") {
        refused.fetch_add(1, Ordering::SeqCst);
        log(format!("404 for {method} from {ip} (wrong path)"));
        return respond(&mut conn, "404 Not Found");
    }
    let ua = header("user-agent").unwrap_or_default();
    let range = header("range").map(|r| format!(", Range {r}")).unwrap_or_default();
    log(format!("{method} from {ip}, allowed: {rule} (User-Agent {ua:?}{range})"));
    let head_only = method == "HEAD";
    if !head_only && active.fetch_add(1, Ordering::SeqCst) >= MAX_CLIENTS {
        active.fetch_sub(1, Ordering::SeqCst);
        log(format!("503 for {ip}: already {MAX_CLIENTS} connections"));
        return respond(&mut conn, "503 Service Unavailable");
    }
    let headers = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: {}\r\nAccess-Control-Allow-Origin: *\r\nCache-Control: no-cache, no-store\r\nAccept-Ranges: none\r\nConnection: close\r\n\r\n",
        format.content_type()
    );
    let _ = conn.set_write_timeout(Some(WRITE_TIMEOUT));
    let _ = conn.set_nodelay(true);
    if conn.write_all(headers.as_bytes()).is_err() || head_only {
        return;
    }
    let rx = hub.subscribe(DEPTH);
    let fetch = Arc::new(Fetch { peer: ip, started: Instant::now(), frames: AtomicU64::new(0), token_gen: my_gen });
    st.lock().unwrap().newest = Some(fetch.clone());
    let why = stream(&mut conn, format, &rx, &fetch, generation, quit);
    active.fetch_sub(1, Ordering::SeqCst);
    log(format!("the stream to {ip} ended after {:.1} s of audio ({why})", fetch.served_secs()));
}

fn stream(conn: &mut TcpStream, format: Format, rx: &mpsc::Receiver<Arc<[u8]>>, fetch: &Fetch, generation: &AtomicU64, quit: &AtomicBool) -> &'static str {
    let mut enc = flac::Encoder::default();
    let start = match format {
        Format::Wav => wav_header(),
        Format::Flac => flac::header(),
    };
    if conn.write_all(&start).is_err() {
        return "closed by the device";
    }
    let mut pending = 0u64; // FLAC: frames waiting for a whole block
    loop {
        if quit.load(Ordering::SeqCst) {
            return "server stopped";
        }
        if generation.load(Ordering::SeqCst) != fetch.token_gen {
            return "a newer URL was loaded";
        }
        let chunk = match rx.recv_timeout(Duration::from_millis(500)) {
            Ok(c) => c,
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => return "no more audio",
        };
        let frames = (chunk.len() / FRAME_BYTES) as u64;
        let (bytes, done) = match format {
            Format::Wav => (chunk.to_vec(), frames),
            Format::Flac => {
                let samples: Vec<i16> = chunk.as_chunks::<2>().0.iter().map(|b| i16::from_le_bytes(*b)).collect();
                let out = enc.push(&samples);
                pending += frames;
                let whole = pending - pending % flac::BLOCK as u64;
                pending -= whole;
                (out, whole)
            }
        };
        if !bytes.is_empty() && conn.write_all(&bytes).is_err() {
            return "closed by the device";
        }
        fetch.frames.fetch_add(done, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn get(addr: SocketAddr, path: &str, method: &str) -> (String, Vec<u8>) {
        let mut c = TcpStream::connect(addr).unwrap();
        write!(c, "{method} {path} HTTP/1.1\r\nHost: x\r\nUser-Agent: test\r\n\r\n").unwrap();
        c.set_read_timeout(Some(Duration::from_millis(600))).unwrap();
        let mut out = Vec::new();
        let mut buf = [0u8; 65536];
        let t = Instant::now();
        while t.elapsed() < Duration::from_millis(800) {
            match c.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => out.extend_from_slice(&buf[..n]),
            }
        }
        let split = out.windows(4).position(|w| w == b"\r\n\r\n").map_or(out.len(), |p| p + 4);
        (String::from_utf8_lossy(&out[..split]).into_owned(), out[split..].to_vec())
    }

    #[test]
    fn only_the_token_path_from_the_device_gets_audio() {
        let hub = Arc::new(Hub::default());
        let lo: IpAddr = "127.0.0.1".parse().unwrap();
        let server = Server::start(lo, 0, Format::Wav, hub.clone(), Allow::device(&[lo], false)).unwrap();
        let url = server.url();
        let path = url.split_once(&server.addr().to_string()).unwrap().1.to_string();
        assert!(path.starts_with("/cast/") && path.ends_with("/stream.wav") && path.len() == "/cast//stream.wav".len() + 32, "{path}");
        let (h, _) = get(server.addr(), "/cast/0123/stream.wav", "GET");
        assert!(h.starts_with("HTTP/1.1 404"), "{h}");
        let (h, body) = get(server.addr(), &path, "HEAD");
        assert!(h.starts_with("HTTP/1.1 200") && h.contains("Content-Type: audio/wav") && body.is_empty(), "{h}");
        // GET: the header, then the live audio from now on.
        let feeder = {
            let hub = hub.clone();
            std::thread::spawn(move || {
                for _ in 0..6 {
                    std::thread::sleep(Duration::from_millis(60));
                    hub.publish(&[1u8; 4 * 1000]);
                }
            })
        };
        let (h, body) = get(server.addr(), &path, "GET");
        feeder.join().unwrap();
        assert!(h.starts_with("HTTP/1.1 200"), "{h}");
        assert_eq!(&body[..44], &wav_header()[..]);
        assert!(body.len() > 44 + 4000 && body[44..].iter().all(|&b| b == 1), "{} bytes", body.len());
        let f = server.newest().expect("the fetch is known");
        assert!(f.served_secs() > 0.0);
        // A new token: the old path is a 404.
        let new = server.rotate();
        assert_ne!(new, url);
        assert!(server.newest().is_none(), "the old fetch is not the current URL's");
        let (h, _) = get(server.addr(), &path, "GET");
        assert!(h.starts_with("HTTP/1.1 404"), "{h}");
        assert_eq!(server.refused.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn another_address_is_refused() {
        let hub = Arc::new(Hub::default());
        let lo: IpAddr = "127.0.0.1".parse().unwrap();
        let server = Server::start(lo, 0, Format::Flac, hub, Allow::device(&["192.168.1.40".parse().unwrap()], false)).unwrap();
        let url = server.url();
        let path = url.split_once(&server.addr().to_string()).unwrap().1.to_string();
        let (h, _) = get(server.addr(), &path, "GET");
        assert!(h.starts_with("HTTP/1.1 403"), "{h}");
        assert_eq!(server.refused.load(Ordering::SeqCst), 1);
        // A group: the members seen by mDNS are let in.
        server.allow(&[lo]);
        let (h, body) = get(server.addr(), &path, "GET");
        assert!(h.starts_with("HTTP/1.1 200") && h.contains("audio/flac"), "{h}");
        assert_eq!(&body[..4], b"fLaC");
    }

    #[test]
    fn a_tv_with_ipv6_privacy_addresses_is_let_in_by_its_64() {
        let ip = |s: &str| s.parse::<IpAddr>().unwrap();
        // What the TV advertised, and the temporary address it fetched from in the live test.
        let tv = [ip("2a01:e11:4039:c270:2b07:8567:abb:8fd8"), ip("fe80::aa:bb"), ip("192.168.1.173")];
        let fetched = ip("2a01:e11:4039:c270:4129:c641:7f11:758f");
        let v6 = Allow::device(&tv, true);
        assert_eq!(v6.rule(ip("2a01:e11:4039:c270:2b07:8567:abb:8fd8")), Some("an address the device advertises"));
        assert_eq!(v6.rule(ip("192.168.1.173")), Some("an address the device advertises"));
        assert_eq!(v6.rule(fetched), Some("in the same /64 as the device"));
        assert_eq!(v6.rule(ip("2a01:e11:4039:c271::1")), None, "another /64");
        assert_eq!(v6.rule(ip("fe80::cc:dd")), None, "link-local is never a /64 rule");
        assert_eq!(v6.rule(ip("192.168.1.174")), None);
        assert!(v6.describe().contains("2a01:e11:4039:c270::/64"), "{}", v6.describe());
        // Served over IPv4: no /64 rule, the advertised addresses only.
        let v4 = Allow::device(&tv, false);
        assert_eq!(v4.rule(fetched), None);
        let mut group = v4.clone();
        group.add_others(&[ip("192.168.1.50")]);
        assert_eq!(group.rule(ip("192.168.1.50")), Some("a Cast device seen on the network (group)"));
    }

    #[test]
    fn addresses_compare_without_the_v6_mapping() {
        let v4: IpAddr = "192.168.1.40".parse().unwrap();
        let mapped: IpAddr = "::ffff:192.168.1.40".parse().unwrap();
        assert!(allowed(mapped, &[v4]) && allowed(v4, &[mapped]));
        assert!(!allowed("192.168.1.41".parse().unwrap(), &[v4]));
        assert_eq!(local_addr_for("127.0.0.1".parse().unwrap()), Some("127.0.0.1".parse().unwrap()));
        assert_ne!(token(), token());
    }
}
