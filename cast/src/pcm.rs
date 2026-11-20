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


pub const RATE: u64 = 44100;
pub const FRAME_BYTES: usize = 4;

fn log(msg: impl AsRef<str>) {
    eprintln!("cast: {}", msg.as_ref());
}

/// How much recent audio the hub keeps for a new subscriber's pre-roll.
pub const HISTORY_FRAMES: u64 = 10 * RATE;

#[derive(Default)]
struct Subs {
    txs: Vec<mpsc::SyncSender<Arc<[u8]>>>,
    /// The last chunks published (at most `HISTORY_FRAMES`), oldest first.
    history: std::collections::VecDeque<Arc<[u8]>>,
    history_frames: u64,
}

/// A subscription with its pre-roll: `pad_frames` of silence come first (the history was
/// shorter than asked), then `backlog_frames` of recent audio already queued in `rx`, then live.
pub struct Sub {
    pub rx: mpsc::Receiver<Arc<[u8]>>,
    pub pad_frames: u64,
    pub backlog_frames: u64,
}

/// The newest chunks that add up to `want` frames (the oldest one cut to fit), oldest first.
pub fn backlog(history: &std::collections::VecDeque<Arc<[u8]>>, want: u64) -> Vec<Arc<[u8]>> {
    let mut out = Vec::new();
    let mut have = 0u64;
    for c in history.iter().rev() {
        if have >= want {
            break;
        }
        let frames = (c.len() / FRAME_BYTES) as u64;
        if have + frames <= want {
            out.push(c.clone());
            have += frames;
        } else {
            let keep = ((want - have) as usize) * FRAME_BYTES;
            out.push(Arc::from(&c[c.len() - keep..]));
            have = want;
        }
    }
    out.reverse();
    out
}

/// The fan-out. Cheap to share (`Arc`).
#[derive(Default)]
pub struct Hub {
    subs: Mutex<Subs>,
    /// Frames published since the hub was made (audio and silence).
    published: AtomicU64,
}

impl Hub {
    /// A new subscriber, from now on. `depth` chunks may wait for it before it loses some.
    pub fn subscribe(&self, depth: usize) -> mpsc::Receiver<Arc<[u8]>> {
        self.subscribe_preroll(depth, 0).rx
    }

    /// A new subscriber that first gets `preroll` frames from just before now (the recent
    /// history, padded with silence when there is less), so the device starts with a buffer.
    pub fn subscribe_preroll(&self, depth: usize, preroll: u64) -> Sub {
        let mut s = self.subs.lock().unwrap();
        let back = backlog(&s.history, preroll.min(HISTORY_FRAMES));
        let backlog_frames: u64 = back.iter().map(|c| (c.len() / FRAME_BYTES) as u64).sum();
        let (tx, rx) = mpsc::sync_channel(depth + back.len());
        for c in back {
            let _ = tx.try_send(c);
        }
        s.txs.push(tx);
        Sub { rx, pad_frames: preroll - backlog_frames.min(preroll), backlog_frames }
    }

    /// Whole frames to every subscriber.
    pub fn publish(&self, bytes: &[u8]) {
        let n = bytes.len() - bytes.len() % FRAME_BYTES;
        if n == 0 {
            return;
        }
        let data: Arc<[u8]> = Arc::from(&bytes[..n]);
        let mut s = self.subs.lock().unwrap();
        s.txs.retain(|tx| !matches!(tx.try_send(data.clone()), Err(mpsc::TrySendError::Disconnected(_))));
        s.history.push_back(data);
        s.history_frames += (n / FRAME_BYTES) as u64;
        while s.history_frames > HISTORY_FRAMES {
            let Some(old) = s.history.pop_front() else { break };
            s.history_frames -= (old.len() / FRAME_BYTES) as u64;
        }
        drop(s);
        self.published.fetch_add((n / FRAME_BYTES) as u64, Ordering::Relaxed);
    }

    pub fn published(&self) -> u64 {
        self.published.load(Ordering::Relaxed)
    }

    /// Drops the recent history: the next pre-roll is silence. After a seek or a skip, the audio
    /// from before it must not play again on the device.
    pub fn forget(&self) {
        let mut s = self.subs.lock().unwrap();
        s.history.clear();
        s.history_frames = 0;
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
    /// Paused on the receiver: nothing goes out (the device's buffer holds what comes next).
    hold: AtomicBool,
    /// Counts the ends of holds: the pump starts its clock afresh (no catch-up for the pause).
    resumes: AtomicU64,
    epoch: Mutex<Option<Instant>>,
}

impl Feed {
    fn now_ms(&self) -> u64 {
        self.epoch.lock().unwrap().get_or_insert_with(Instant::now).elapsed().as_millis() as u64
    }

    pub(crate) fn set_casting(&self, on: bool) {
        self.casting.store(on, Ordering::SeqCst);
    }

    pub(crate) fn set_hold(&self, on: bool) {
        if self.hold.swap(on, Ordering::SeqCst) && !on {
            self.resumes.fetch_add(1, Ordering::SeqCst);
            self.last_audio.store(self.now_ms().max(1), Ordering::Relaxed);
        }
    }

    pub fn holding(&self) -> bool {
        self.hold.load(Ordering::SeqCst)
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
                        if !whole.is_empty() && !f.local_active.load(Ordering::SeqCst) && !f.holding() {
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

/// Nobody has sent audio for this long (paused, between songs): silence.
const SILENCE_GRACE: Duration = Duration::from_millis(250);
/// How far ahead of the clock silence is written.
const SILENCE_LEAD: Duration = Duration::from_millis(100);
/// A longer hole than this (the computer slept) is skipped, not filled.
const MAX_CATCH_UP: Duration = Duration::from_secs(2);

/// The silence to write now, and where the stream then ends. Unlike AirPlay's rule this catches
/// up: silence starts where the last audio ended (not "now"), and a late wake-up fills what it
/// missed. For Cast this matters: the device plays from its buffer, and every hole in the stream
/// is taken from that buffer for good (a few pauses would drain it into an underrun).
pub fn silence_after(now: Instant, last_audio: Instant, until: Instant) -> (usize, Instant) {
    if now.saturating_duration_since(last_audio) < SILENCE_GRACE {
        return (0, until);
    }
    let want = now + SILENCE_LEAD;
    let mut from = until.max(last_audio);
    if from >= want {
        return (0, from);
    }
    if want.duration_since(from) > MAX_CATCH_UP {
        from = now;
    }
    let gap = want.duration_since(from);
    let frames = (gap.as_secs_f64() * RATE as f64).round() as usize;
    (frames, from + Duration::from_secs_f64(frames as f64 / RATE as f64))
}

fn to_bytes(samples: &[i16]) -> Vec<u8> {
    samples.iter().flat_map(|s| s.to_le_bytes()).collect()
}

/// The local player's chunks while it is in charge, and silence whenever nobody sent audio for a
/// moment (as AirPlay's pump).
fn pump(rx: mpsc::Receiver<Vec<i16>>, feed: &Feed, hub: &Hub, alive: &AtomicBool) {
    let epoch = *feed.epoch.lock().unwrap().get_or_insert_with(Instant::now);
    feed.last_audio.store(0, Ordering::Relaxed); // a new session: nothing sent yet
    let mut until = Instant::now();
    let mut resumes = feed.resumes.load(Ordering::SeqCst);
    while alive.load(Ordering::SeqCst) {
        let chunk = match rx.recv_timeout(Duration::from_millis(20)) {
            Ok(c) => Some(c),
            Err(mpsc::RecvTimeoutError::Timeout) => None,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        };
        let now = Instant::now();
        if feed.holding() {
            continue; // paused on the receiver: nothing, not even silence
        }
        let r = feed.resumes.load(Ordering::SeqCst);
        if r != resumes {
            resumes = r;
            until = now; // the pause is not a hole to fill
        }
        match chunk {
            Some(c) if c.len() >= 2 && feed.local_active.load(Ordering::SeqCst) => {
                let c = &c[..c.len() - c.len() % 2];
                until = until.max(now) + Duration::from_secs_f64((c.len() / 2) as f64 / RATE as f64);
                hub.publish(&to_bytes(c));
                feed.last_audio.store(feed.now_ms(), Ordering::Relaxed);
            }
            Some(_) => {}
            None => {
                // Nothing sent yet this session: silence at once (no grace), so the stream starts
                // when the device fetches; a gap here would be lost from the device's lead for good.
                let last = match feed.last_audio.load(Ordering::Relaxed) {
                    0 => now.checked_sub(Duration::from_secs(1)).unwrap_or(epoch),
                    ms => epoch + Duration::from_millis(ms),
                };
                let (frames, end) = silence_after(now, last.min(now), until);
                until = end;
                if frames > 0 {
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
        assert_eq!(hub.subs.lock().unwrap().txs.len(), 1, "a gone subscriber is forgotten");
    }

    #[test]
    fn a_new_subscriber_gets_its_preroll_from_the_history_then_silence_for_the_rest() {
        let hub = Hub::default();
        // 3 chunks of 1000 frames, values 1, 2, 3.
        for v in 1..=3u8 {
            hub.publish(&vec![v; 1000 * FRAME_BYTES]);
        }
        let s = hub.subscribe_preroll(8, 2500);
        assert_eq!((s.pad_frames, s.backlog_frames), (0, 2500));
        let got: Vec<Arc<[u8]>> = s.rx.try_iter().collect();
        assert_eq!(got.iter().map(|c| c.len() / FRAME_BYTES).collect::<Vec<_>>(), vec![500, 1000, 1000], "the oldest cut to fit");
        assert_eq!((got[0][0], got[1][0], got[2][0]), (1, 2, 3));
        // More than the history holds: silence first for what is missing.
        let s = hub.subscribe_preroll(8, 5000);
        assert_eq!((s.pad_frames, s.backlog_frames), (2000, 3000));
        // No pre-roll: from now on only.
        let s = hub.subscribe_preroll(8, 0);
        assert!(s.rx.try_recv().is_err());
        hub.publish(&[4; FRAME_BYTES]);
        assert_eq!(s.rx.try_recv().unwrap()[0], 4);
    }

    #[test]
    fn silence_catches_up_from_where_the_audio_ended() {
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let frames = |ms: u64| (ms * RATE / 1000) as usize;
        // Audio until 1000 ms, then the bridge stops (paused): nothing within the grace.
        assert_eq!(silence_after(at(1200), at(1000), at(0)), (0, at(0)));
        // After the grace: silence from 1000 ms on, up to the lead (no hole in the stream).
        let (n, end) = silence_after(at(1300), at(1000), at(0));
        assert_eq!((n, end), (frames(400), at(1400)));
        // On time: 20 ms later, 20 ms more.
        assert_eq!(silence_after(at(1320), at(1000), end).0, frames(20));
        // Woken 300 ms late: the missed time is filled.
        assert_eq!(silence_after(at(1620), at(1000), at(1420)).0, frames(300));
        // Ahead already: nothing.
        assert_eq!(silence_after(at(1320), at(1000), at(1500)), (0, at(1500)));
        // After a sleep of 10 s: skipped, the lead only.
        assert_eq!(silence_after(at(11_420), at(1000), at(1420)), (frames(100), at(11_520)));
    }

    #[test]
    fn a_hold_sends_nothing_and_its_end_is_no_hole_to_fill() {
        let feed = Arc::new(Feed::default());
        let hub = Arc::new(Hub::default());
        let rx = hub.subscribe(1024);
        let dir = std::env::temp_dir().join(format!("uzumaki-cast-test-h-{}", std::process::id()));
        let inputs = Inputs::start(&dir.join("pcm.sock"), feed.clone(), hub.clone());
        let local = LocalFeed { feed: feed.clone() };
        std::thread::sleep(Duration::from_millis(300));
        let _ = rx.try_iter().count();
        feed.set_hold(true);
        std::thread::sleep(Duration::from_millis(60));
        let _ = rx.try_iter().count(); // what was on its way
        local.set_active(true);
        local.push(vec![100i16; 2 * 4096]); // the local player while paused: dropped
        std::thread::sleep(Duration::from_millis(600));
        assert_eq!(rx.try_iter().map(|c| c.len()).sum::<usize>(), 0, "nothing while held");
        local.set_active(false);
        feed.set_hold(false);
        std::thread::sleep(Duration::from_millis(500));
        // After the hold: back to real time, not 600 ms of catch-up silence at once.
        let after: usize = rx.try_iter().map(|c| c.len()).sum();
        // 500 ms + the 100 ms lead in real time; a catch-up of the 600 ms hold would be ~1.2 s.
        assert!(after > 0 && after < 44100 * 4 * 900 / 1000, "{after} bytes in 500 ms after the hold");
        drop(inputs);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_history_is_bounded() {
        let hub = Hub::default();
        let chunk = vec![0u8; 44100 * FRAME_BYTES]; // 1 s
        for _ in 0..15 {
            hub.publish(&chunk);
        }
        let s = hub.subs.lock().unwrap();
        assert!(s.history_frames <= HISTORY_FRAMES && s.history_frames >= HISTORY_FRAMES - 44100, "{}", s.history_frames);
    }

    #[test]
    fn after_forget_the_preroll_is_silence() {
        let hub = Hub::default();
        hub.publish(&[7u8; 4410 * FRAME_BYTES]);
        hub.forget();
        let sub = hub.subscribe_preroll(4, 4410);
        assert_eq!((sub.pad_frames, sub.backlog_frames), (4410, 0), "nothing from before the seek");
        assert!(sub.rx.try_recv().is_err());
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
        // Silence may come first (it starts with the session); then the bridge's whole frames.
        let at = bytes.windows(8).position(|w| w == [9, 9, 9, 9, 1, 1, 1, 1]);
        assert!(at.is_some_and(|i| i % FRAME_BYTES == 0), "the bridge's frames, whole and aligned: {:?}", &bytes[bytes.len().saturating_sub(16)..]);
        let _ = std::fs::remove_dir_all(dir);
    }
}
