//! The audio for the Cast device: s16le stereo 44.1 kHz frames, in memory only.
//!
//! [`Hub`] hands every chunk published to each subscriber (an HTTP connection of the device). A
//! subscriber that doesn't keep up loses chunks; the source never waits for it.
//!
//! [`Inputs`] is the app's side while a session runs, the same way in as AirPlay's: the bridge's
//! capture on a Unix socket of the session, the local player through a [`LocalFeed`], and silence
//! whenever neither sends anything (paused, between songs), so the device's stream never stalls.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use uzumaki_airplay::silence_due;

pub const RATE: u64 = 44100;
pub const FRAME_BYTES: usize = 4;

fn log(msg: impl AsRef<str>) {
    eprintln!("cast: {}", msg.as_ref());
}

/// The fan-out. Cheap to share (`Arc`).
#[derive(Default)]
pub struct Hub {
    subs: Mutex<Vec<mpsc::SyncSender<Arc<[u8]>>>>,
    /// Frames published since the hub was made (audio and silence).
    published: AtomicU64,
}

impl Hub {
    /// A new subscriber, from now on. `depth` chunks may wait for it before it loses some.
    pub fn subscribe(&self, depth: usize) -> mpsc::Receiver<Arc<[u8]>> {
        let (tx, rx) = mpsc::sync_channel(depth);
        self.subs.lock().unwrap().push(tx);
        rx
    }

    /// Whole frames to every subscriber.
    pub fn publish(&self, bytes: &[u8]) {
        let n = bytes.len() - bytes.len() % FRAME_BYTES;
        if n == 0 {
            return;
        }
        let data: Arc<[u8]> = Arc::from(&bytes[..n]);
        self.subs.lock().unwrap().retain(|tx| !matches!(tx.try_send(data.clone()), Err(mpsc::TrySendError::Disconnected(_))));
        self.published.fetch_add((n / FRAME_BYTES) as u64, Ordering::Relaxed);
    }

    pub fn published(&self) -> u64 {
        self.published.load(Ordering::Relaxed)
    }
}

/// What the players and the session share.
#[derive(Default)]
pub struct Feed {
    /// A session takes audio (the local outputs go quiet).
    casting: AtomicBool,
    keep_local: AtomicBool,
    /// The local player has a song loaded: its audio (or silence) goes out, the bridge's doesn't.
    local_active: AtomicBool,
    /// The bridge is connected to the session's socket.
    pub(crate) bridge_linked: AtomicBool,
    /// The bridge said its capture taps the player.
    pub(crate) bridge_ready: AtomicBool,
    /// The app runs no bridge (local only).
    pub(crate) no_bridge: AtomicBool,
    local_tx: Mutex<Option<mpsc::SyncSender<Vec<i16>>>>,
    /// Milliseconds since `epoch` of the last audio (not silence) published.
    last_audio: AtomicU64,
    epoch: Mutex<Option<Instant>>,
}

impl Feed {
    fn now_ms(&self) -> u64 {
        self.epoch.lock().unwrap().get_or_insert_with(Instant::now).elapsed().as_millis() as u64
    }

    pub(crate) fn set_casting(&self, on: bool) {
        self.casting.store(on, Ordering::SeqCst);
    }

    pub(crate) fn set_keep_local(&self, on: bool) {
        self.keep_local.store(on, Ordering::SeqCst);
    }

    /// What the device gets is what the user hears.
    pub fn source_ready(&self) -> bool {
        self.local_active.load(Ordering::SeqCst) || self.no_bridge.load(Ordering::SeqCst) || (self.bridge_ready.load(Ordering::SeqCst) && self.bridge_linked.load(Ordering::SeqCst))
    }
}

/// The local file player's way to the device (the same calls as AirPlay's `LocalFeed`).
#[derive(Clone)]
pub struct LocalFeed {
    pub(crate) feed: Arc<Feed>,
}

impl LocalFeed {
    pub fn casting(&self) -> bool {
        self.feed.casting.load(Ordering::Relaxed)
    }
    pub fn mute_local(&self) -> bool {
        self.casting() && !self.feed.keep_local.load(Ordering::Relaxed)
    }
    /// Interleaved s16 stereo 44.1 kHz. Never waits (audio thread): dropped when the queue is full.
    pub fn push(&self, chunk: Vec<i16>) {
        if let Some(tx) = self.feed.local_tx.lock().unwrap().as_ref() {
            let _ = tx.try_send(chunk);
        }
    }
    pub fn set_active(&self, on: bool) {
        if self.feed.local_active.swap(on, Ordering::SeqCst) != on && self.casting() {
            log(if on { "local player in charge: the device gets the local file" } else { "bridge in charge: the device gets Apple Music" });
        }
    }
}

/// The session's inputs; they end when dropped.
pub struct Inputs {
    alive: Arc<AtomicBool>,
    socket: Option<PathBuf>,
    threads: Vec<std::thread::JoinHandle<()>>,
}

impl Inputs {
    /// Binds `socket` for the bridge (Unix only) and starts the local/silence pump.
    pub fn start(socket: &Path, feed: Arc<Feed>, hub: Arc<Hub>) -> Inputs {
        let alive = Arc::new(AtomicBool::new(true));
        let mut threads = Vec::new();
        let (ltx, lrx) = mpsc::sync_channel::<Vec<i16>>(32);
        *feed.local_tx.lock().unwrap() = Some(ltx);
        {
            let (f, h, a) = (feed.clone(), hub.clone(), alive.clone());
            threads.push(std::thread::Builder::new().name("cast-local".into()).spawn(move || pump(lrx, &f, &h, &a)).expect("spawn cast-local"));
        }
        let bound = bridge_socket(socket, &feed, &hub, &alive, &mut threads);
        Inputs { alive, socket: bound.then(|| socket.to_path_buf()), threads }
    }
}

#[cfg(unix)]
fn bridge_socket(path: &Path, feed: &Arc<Feed>, hub: &Arc<Hub>, alive: &Arc<AtomicBool>, threads: &mut Vec<std::thread::JoinHandle<()>>) -> bool {
    use std::io::Read;
    use std::os::unix::net::UnixListener;
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
        }
    }
    let _ = std::fs::remove_file(path);
    let listener = match UnixListener::bind(path) {
        Ok(l) => l,
        Err(e) => {
            log(format!("can't bind the PCM socket {}: {e}", path.display()));
            return false;
        }
    };
    let _ = listener.set_nonblocking(true);
    let (f, h, a) = (feed.clone(), hub.clone(), alive.clone());
    let t = std::thread::Builder::new().name("cast-pcm".into()).spawn(move || {
        let mut buf = vec![0u8; 16384];
        while a.load(Ordering::SeqCst) {
            let mut s = match listener.accept() {
                Ok((s, _)) => s,
                Err(_) => {
                    std::thread::sleep(Duration::from_millis(50));
                    continue;
                }
            };
            let _ = s.set_nonblocking(false);
            let _ = s.set_read_timeout(Some(Duration::from_millis(200)));
            f.bridge_linked.store(true, Ordering::SeqCst);
            log("the bridge is connected to the PCM socket");
            let mut frames = uzumaki_airplay::Frames::default();
            let mut total = 0u64;
            while a.load(Ordering::SeqCst) {
                match s.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        let whole = frames.whole(&buf[..n]);
                        if !whole.is_empty() && !f.local_active.load(Ordering::SeqCst) {
                            h.publish(whole);
                            total += whole.len() as u64;
                            f.last_audio.store(f.now_ms(), Ordering::Relaxed);
                        }
                        frames.consume();
                    }
                    Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut | std::io::ErrorKind::Interrupted) => {}
                    Err(_) => break,
                }
            }
            f.bridge_linked.store(false, Ordering::SeqCst);
            log(format!("the bridge's PCM stream ended after {:.1} s", total as f64 / (RATE as f64 * FRAME_BYTES as f64)));
        }
    });
    match t {
        Ok(t) => threads.push(t),
        Err(e) => log(format!("can't start the PCM thread: {e}")),
    }
    true
}

#[cfg(not(unix))]
fn bridge_socket(_: &Path, _: &Arc<Feed>, _: &Arc<Hub>, _: &Arc<AtomicBool>, _: &mut Vec<std::thread::JoinHandle<()>>) -> bool {
    log("no PCM socket on this system yet: only local files reach the device");
    false
}

impl Drop for Inputs {
    fn drop(&mut self) {
        self.alive.store(false, Ordering::SeqCst);
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
        if let Some(p) = &self.socket {
            let _ = std::fs::remove_file(p);
        }
    }
}

fn to_bytes(samples: &[i16]) -> Vec<u8> {
    samples.iter().flat_map(|s| s.to_le_bytes()).collect()
}

/// The local player's chunks while it is in charge, and silence whenever nobody sent audio for a
/// moment (as AirPlay's pump).
fn pump(rx: mpsc::Receiver<Vec<i16>>, feed: &Feed, hub: &Hub, alive: &AtomicBool) {
    let epoch = *feed.epoch.lock().unwrap().get_or_insert_with(Instant::now);
    let mut until = Instant::now();
    while alive.load(Ordering::SeqCst) {
        let chunk = match rx.recv_timeout(Duration::from_millis(20)) {
            Ok(c) => Some(c),
            Err(mpsc::RecvTimeoutError::Timeout) => None,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        };
        let now = Instant::now();
        match chunk {
            Some(c) if c.len() >= 2 && feed.local_active.load(Ordering::SeqCst) => {
                let c = &c[..c.len() - c.len() % 2];
                until = until.max(now) + Duration::from_secs_f64((c.len() / 2) as f64 / RATE as f64);
                hub.publish(&to_bytes(c));
                feed.last_audio.store(feed.now_ms(), Ordering::Relaxed);
            }
            Some(_) => {}
            None => {
                let last = epoch + Duration::from_millis(feed.last_audio.load(Ordering::Relaxed));
                let frames = silence_due(now, last.min(now), until);
                if frames > 0 {
                    until = until.max(now) + Duration::from_secs_f64(frames as f64 / RATE as f64);
                    hub.publish(&vec![0u8; frames * FRAME_BYTES]);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_subscriber_gets_whole_frames_and_a_slow_one_never_blocks() {
        let hub = Hub::default();
        let a = hub.subscribe(8);
        let slow = hub.subscribe(1);
        hub.publish(&[1, 2, 3, 4, 5, 6]); // one frame and a half: the half is dropped
        hub.publish(&[7, 8, 9, 10]);
        assert_eq!(&*a.recv().unwrap(), &[1, 2, 3, 4]);
        assert_eq!(&*a.recv().unwrap(), &[7, 8, 9, 10]);
        assert_eq!(&*slow.recv().unwrap(), &[1, 2, 3, 4]);
        assert!(slow.try_recv().is_err(), "the slow one lost the second chunk");
        assert_eq!(hub.published(), 2);
        drop(a);
        hub.publish(&[0; 4]);
        assert_eq!(hub.subs.lock().unwrap().len(), 1, "a gone subscriber is forgotten");
    }

    #[test]
    fn silence_keeps_the_stream_going_and_the_local_player_takes_over() {
        let feed = Arc::new(Feed::default());
        let hub = Arc::new(Hub::default());
        let rx = hub.subscribe(256);
        let dir = std::env::temp_dir().join(format!("uzumaki-cast-test-{}", std::process::id()));
        let inputs = Inputs::start(&dir.join("pcm.sock"), feed.clone(), hub.clone());
        let local = LocalFeed { feed: feed.clone() };
        std::thread::sleep(Duration::from_millis(700));
        let quiet: usize = rx.try_iter().map(|c| c.len()).sum();
        // ~0.45 s of silence after the 250 ms grace (plus the lead); generous bounds for slow CI.
        assert!(quiet > 8_000 && quiet < 200_000, "silence: {quiet} bytes");
        assert!(rx.try_iter().all(|c| c.iter().all(|&b| b == 0)));
        local.set_active(true);
        local.push(vec![100i16; 2 * 4096]);
        std::thread::sleep(Duration::from_millis(100));
        let got: Vec<Arc<[u8]>> = rx.try_iter().collect();
        assert!(got.iter().any(|c| c.len() == 4096 * 4 && c[..2] == 100i16.to_le_bytes()), "the local chunk went out");
        drop(inputs);
        #[cfg(unix)]
        assert!(!dir.join("pcm.sock").exists(), "the socket is removed");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[cfg(unix)]
    #[test]
    fn the_bridges_bytes_go_out_unless_the_local_player_is_in_charge() {
        use std::io::Write;
        let feed = Arc::new(Feed::default());
        let hub = Arc::new(Hub::default());
        let rx = hub.subscribe(256);
        let dir = std::env::temp_dir().join(format!("uzumaki-cast-test-b-{}", std::process::id()));
        let path = dir.join("pcm.sock");
        let _inputs = Inputs::start(&path, feed.clone(), hub.clone());
        let mut s = std::os::unix::net::UnixStream::connect(&path).unwrap();
        s.write_all(&[9, 9, 9]).unwrap(); // a frame split over two writes
        s.write_all(&[9, 1, 1, 1, 1]).unwrap();
        let t = Instant::now();
        while !feed.bridge_linked.load(Ordering::SeqCst) && t.elapsed() < Duration::from_secs(2) {
            std::thread::sleep(Duration::from_millis(10));
        }
        std::thread::sleep(Duration::from_millis(150));
        let bytes: Vec<u8> = rx.try_iter().flat_map(|c| c.to_vec()).collect();
        assert!(bytes.starts_with(&[9, 9, 9, 9, 1, 1, 1, 1]), "{bytes:?}");
        let _ = std::fs::remove_dir_all(dir);
    }
}
