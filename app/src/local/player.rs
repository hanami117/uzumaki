//! In-process player for local files. CONTRACT (owned by the main session; the bodies are the
//! local-files agent's job):
//!
//! - One thread ("local-player") owns the audio output. It opens the default output device
//!   lazily on the first `play` and closes it on `stop`, so headless runs, tests and an app
//!   that never plays a local file never touch the sound card.
//! - Every `play` carries a generation number from the app. Every event the thread sends is
//!   tagged with the generation of the song it is about; the app drops events of older
//!   generations (a late `Ended` must never skip the song the user just clicked).
//! - Events, through `on_event(gen, event)`, in this order for a normal song:
//!   `NowPlaying(Some(song))`, `PlayState(Loading)`, `PlayState(Playing)`, then
//!   `Time { current, duration }` about 4 times a second while playing, and finally
//!   `PlayState(Ended)` once when the file has played to its end. `pause`/`resume` send
//!   `PlayState(Paused)`/`PlayState(Playing)`. A file that can't be opened or decoded sends
//!   `Error("Couldn't play “Title”: …")` then `PlayState(Stopped)`.
//! - `active` is true from `play` until `stop` (shared with `main.rs`).
//! - The output follows the system: when the default device changes (headphones plugged in,
//!   AirPods taken), or the device stops taking audio (unplugged: the position stops moving for
//!   `STALL_TICKS`), the song moves to the default device where it was. With no device left it
//!   pauses (`PlayState(Paused)`); Play tries again.
//! - `UZUMAKI_MUTE=1` (the agents' test switch, as for the bridge; the old `CADENCE_MUTE=1`
//!   still works) forces volume 0.
//! - AirPlay: the decoder is wrapped in a `tap::Tap` that copies the audio to the speaker
//!   session and silences this computer while a speaker takes it
//!   (the volume setting stays as it is). The thread tells the session whether a song is loaded,
//!   so one speaker session follows the hand-overs between this player and the bridge.

use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::time::{Duration, Instant};

use rodio::{Decoder, OutputStream, OutputStreamBuilder, Sink, Source};

use super::tap::{CastSink, Gain, Tap};
use crate::bridge::{Event, PlayState};
use crate::models::Song;

/// Commands to the player thread.
#[derive(Debug)]
pub enum Cmd {
    /// Start `path` from `start_at` seconds at `volume` (0..1), replacing whatever plays.
    Play { generation: u64, song: Song, path: PathBuf, start_at: f64, volume: f64 },
    Pause,
    Resume,
    /// Seconds from the start.
    Seek(f64),
    /// 0..1, linear (as MusicKit's volume).
    Volume(f64),
    /// Stop and close the output device.
    Stop,
}

/// Handle to the player thread. Cheap to clone.
#[derive(Clone)]
pub struct LocalPlayer {
    tx: Sender<Cmd>,
    active: Arc<AtomicBool>,
    /// The tap's gain: its hold is set here, on the caller's thread, before a quiet seek is sent.
    gain: Gain,
}

impl LocalPlayer {
    /// Starts the thread (it sleeps until the first command). `active` is shared with `main.rs`.
    pub fn spawn(on_event: impl Fn(u64, Event) + Send + 'static, active: Arc<AtomicBool>) -> Self {
        let (tx, rx) = mpsc::channel::<Cmd>();
        let gain = Gain::new(1.0);
        let g = gain.clone();
        std::thread::Builder::new()
            .name("local-player".into())
            .spawn(move || run(rx, on_event, g))
            .expect("spawn local-player thread");
        Self { tx, active, gain }
    }

    pub fn play(&self, generation: u64, song: &Song, start_at: f64, volume: f64) {
        let Some(path) = song.local_path() else { return };
        self.active.store(true, Ordering::SeqCst);
        let _ = self.tx.send(Cmd::Play { generation, song: song.clone(), path: path.to_path_buf(), start_at, volume });
    }

    pub fn pause(&self) {
        let _ = self.tx.send(Cmd::Pause);
    }

    pub fn resume(&self) {
        let _ = self.tx.send(Cmd::Resume);
    }

    pub fn seek(&self, seconds: f64) {
        let _ = self.tx.send(Cmd::Seek(seconds));
    }

    /// A seek nobody may hear on its way: this computer stays silent from now until it has landed
    /// (then fades in). Back from the speakers, the old place would be heard for a moment otherwise.
    pub fn seek_quiet(&self, seconds: f64) {
        self.gain.hold(true);
        if self.tx.send(Cmd::Seek(seconds)).is_err() {
            self.gain.hold(false);
        }
    }

    pub fn set_volume(&self, volume: f64) {
        let _ = self.tx.send(Cmd::Volume(volume));
    }

    pub fn stop(&self) {
        self.active.store(false, Ordering::SeqCst);
        let _ = self.tx.send(Cmd::Stop);
    }
}

/// How often the thread wakes up to report the position and look for the end of the file.
const TICK: Duration = Duration::from_millis(250);
/// While playing, the default output device is looked at every this many ticks (2 s).
const DEVICE_TICKS: u32 = 8;
/// Playing, but the position hasn't moved for this many ticks (3 s): the device stopped taking
/// audio (unplugged, Bluetooth gone). The song moves to the default device.
const STALL_TICKS: u32 = 12;
/// Reopens for a stall before giving up (paused, `Session::lost`).
const MAX_STALLS: u32 = 2;

/// Where sinks come from: the real sound card, or (in tests) a device-less queue.
trait Output {
    /// A fresh sink, opening the device when it isn't open yet (or isn't the default any more).
    fn sink(&mut self) -> Result<Sink, String>;
    /// Closes the device.
    fn close(&mut self);
    /// Whether the system's default device is another one than the device open now.
    fn moved(&mut self) -> bool;
}

/// The default output device, opened lazily.
#[derive(Default)]
struct DeviceOutput {
    stream: Option<OutputStream>,
    /// The name the default device had when the stream was opened.
    device: Option<String>,
}

/// The system's default output device's name, None when there is none (or no name).
fn default_device() -> Option<String> {
    use rodio::cpal::traits::{DeviceTrait, HostTrait};
    rodio::cpal::default_host().default_output_device()?.name().ok()
}

impl Output for DeviceOutput {
    fn sink(&mut self) -> Result<Sink, String> {
        if self.stream.is_some() && self.moved() {
            self.close();
        }
        if self.stream.is_none() {
            let mut stream = OutputStreamBuilder::open_default_stream().map_err(|e| format!("no audio output ({e})"))?;
            stream.log_on_drop(false);
            self.stream = Some(stream);
            self.device = default_device();
        }
        Ok(Sink::connect_new(self.stream.as_ref().expect("just opened").mixer()))
    }

    fn close(&mut self) {
        self.stream = None;
        self.device = None;
    }

    fn moved(&mut self) -> bool {
        // No name (or no device at all) says nothing: the stall check covers a device that's gone.
        self.stream.is_some() && default_device().is_some_and(|d| self.device.as_ref() != Some(&d))
    }
}

/// The song that is loaded.
struct Session {
    generation: u64,
    sink: Sink,
    duration: f64,
    ended: bool,
    /// Where the decoder was seeked before it joined the sink (the sink counts from there).
    offset: f64,
    /// What plays, to open it again on another device.
    song: Song,
    path: PathBuf,
    /// The position at the last tick and for how many ticks it hasn't moved while playing.
    last_pos: f64,
    still: u32,
    /// Reopens for a stall in a row without the position moving since.
    stalls: u32,
    /// The device went away and no other could be opened: paused until Play tries again.
    lost: bool,
}

impl Session {
    fn position(&self) -> f64 {
        let current = self.offset + self.sink.get_pos().as_secs_f64();
        if self.duration > 0.0 { current.min(self.duration) } else { current }
    }
}

/// The playing logic, independent of the sound card.
struct Engine<O: Output, E: Fn(u64, Event)> {
    output: O,
    emit: E,
    session: Option<Session>,
    volume: f64,
    mute: bool,
    /// AirPlay: where the tapped audio goes, and the app's volume for the copy.
    tap: Option<Arc<dyn CastSink>>,
    tap_gain: Gain,
    /// Ticks since the default device was last looked at.
    device_ticks: u32,
}

/// Opens `path` for rodio: byte length and extension hint help seeking and m4a.
fn open_decoder(path: &Path) -> Result<Decoder<BufReader<File>>, String> {
    let file = File::open(path).map_err(|e| e.to_string())?;
    let len = file.metadata().map_err(|e| e.to_string())?.len();
    let mut builder = Decoder::builder().with_data(BufReader::new(file)).with_byte_len(len).with_seekable(true);
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        builder = builder.with_hint(&ext.to_ascii_lowercase());
    }
    builder.build().map_err(|e| e.to_string())
}

impl<O: Output, E: Fn(u64, Event)> Engine<O, E> {
    fn new(output: O, emit: E, mute: bool) -> Self {
        Self { output, emit, session: None, volume: 1.0, mute, tap: None, tap_gain: Gain::new(1.0), device_ticks: 0 }
    }

    fn with_tap(mut self, tap: Option<Arc<dyn CastSink>>) -> Self {
        self.tap = tap;
        self
    }

    fn gain(&self) -> f32 {
        if self.mute { 0.0 } else { self.volume.clamp(0.0, 1.0) as f32 }
    }

    fn handle(&mut self, cmd: Cmd) {
        self.handle_cmd(cmd);
        self.tap_gain.set(self.volume as f32);
        if let Some(t) = &self.tap {
            t.set_active(self.session.is_some());
        }
    }

    fn handle_cmd(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::Play { generation, song, path, start_at, volume } => self.play(generation, song, &path, start_at, volume),
            Cmd::Pause => {
                if let Some(s) = self.session.as_ref().filter(|s| !s.ended) {
                    s.sink.pause();
                    (self.emit)(s.generation, Event::PlayState(PlayState::Paused));
                }
            }
            Cmd::Resume => {
                let Some(s) = self.session.as_ref().filter(|s| !s.ended) else { return };
                // Lost, or the default device changed while paused: play on the default one.
                if s.lost || self.output.moved() {
                    return self.reopen(false, 0);
                }
                if let Some(s) = self.session.as_mut() {
                    s.still = 0;
                    s.sink.play();
                    (self.emit)(s.generation, Event::PlayState(PlayState::Playing));
                }
            }
            Cmd::Seek(secs) => {
                if let Some(s) = self.session.as_mut().filter(|s| !s.ended) {
                    let target = if s.duration > 0.0 { secs.min(s.duration) } else { secs }.max(0.0);
                    // The sink's position is absolute after its own seek.
                    if s.sink.try_seek(Duration::from_secs_f64(target)).is_ok() {
                        s.offset = 0.0;
                    }
                    s.last_pos = target;
                    s.still = 0;
                }
                // `try_seek` returns once the audio thread has taken the seek: a quiet seek is over.
                self.tap_gain.hold(false);
                self.send_time();
            }
            Cmd::Volume(v) => {
                self.volume = v;
                if let Some(s) = &self.session {
                    s.sink.set_volume(self.gain());
                }
            }
            Cmd::Stop => {
                self.session = None;
                self.output.close();
            }
        }
    }

    fn play(&mut self, generation: u64, song: Song, path: &Path, start_at: f64, volume: f64) {
        if let Some(old) = self.session.take() {
            old.sink.stop();
        }
        self.volume = volume;
        (self.emit)(generation, Event::NowPlaying(Some(song.clone())));
        (self.emit)(generation, Event::PlayState(PlayState::Loading));
        match self.load(generation, &song, path, start_at) {
            Ok(session) => {
                self.session = Some(session);
                (self.emit)(generation, Event::PlayState(PlayState::Playing));
                self.send_time();
            }
            Err(e) => {
                (self.emit)(generation, Event::Error(format!("Couldn't play “{}”: {e}", song.title)));
                (self.emit)(generation, Event::PlayState(PlayState::Stopped));
            }
        }
    }

    fn load(&mut self, generation: u64, song: &Song, path: &Path, start_at: f64) -> Result<Session, String> {
        let mut decoder = open_decoder(path)?;
        let duration = if song.duration > 0.0 {
            song.duration
        } else {
            decoder.total_duration().map_or(0.0, |d| d.as_secs_f64())
        };
        let sink = self.output.sink()?;
        sink.pause(); // no blip from the start while we seek
        sink.set_volume(self.gain());
        // Seek the decoder itself: `Sink::try_seek` waits for the audio thread.
        let offset = if start_at > 0.0 && decoder.try_seek(Duration::from_secs_f64(start_at)).is_ok() { start_at } else { 0.0 };
        match &self.tap {
            Some(t) => {
                self.tap_gain.set(self.volume as f32);
                sink.append(Tap::new(decoder, t.clone(), self.tap_gain.clone()));
            }
            None => sink.append(decoder),
        }
        sink.play();
        let song = song.clone();
        Ok(Session { generation, sink, duration, ended: false, offset, song, path: path.to_path_buf(), last_pos: offset, still: 0, stalls: 0, lost: false })
    }

    fn send_time(&self) {
        if let Some(s) = &self.session {
            (self.emit)(s.generation, Event::Time { current: s.position(), duration: s.duration });
        }
    }

    /// Moves the song to the default device, where it was: paused if it was paused, or playing
    /// (`keep_paused` false: Play). No device: paused and `lost`, until Play tries again.
    fn reopen(&mut self, keep_paused: bool, stalls: u32) {
        let Some(old) = self.session.take() else { return };
        let at = if old.lost { old.last_pos } else { old.position() };
        let paused = keep_paused && old.sink.is_paused();
        old.sink.stop();
        self.output.close();
        match self.load(old.generation, &old.song, &old.path, at) {
            Ok(mut s) => {
                // The new decoder starts at `at` (or at 0 when it can't seek: then it says so).
                if paused {
                    s.sink.pause();
                }
                s.stalls = stalls;
                let generation = s.generation;
                self.session = Some(s);
                if !keep_paused {
                    (self.emit)(generation, Event::PlayState(PlayState::Playing));
                }
                self.send_time();
            }
            Err(_) => {
                let generation = old.generation;
                let was_lost = old.lost;
                self.session = Some(Session { lost: true, last_pos: at, still: 0, ..old });
                if !was_lost || !keep_paused {
                    (self.emit)(generation, Event::PlayState(PlayState::Paused));
                }
            }
        }
    }

    /// Periodic work: the position, `Ended` (once) when the sink ran dry, and the device.
    fn tick(&mut self) {
        let Some(s) = self.session.as_mut() else { return };
        if s.ended || s.lost || s.sink.is_paused() {
            return;
        }
        if s.sink.empty() {
            s.ended = true;
            let generation = s.generation;
            (self.emit)(generation, Event::PlayState(PlayState::Ended));
            return;
        }
        let pos = s.position();
        if (pos - s.last_pos).abs() < 1e-3 {
            s.still += 1;
        } else {
            s.still = 0;
            s.stalls = 0;
        }
        s.last_pos = pos;
        let stalled = s.still >= STALL_TICKS;
        if stalled && s.stalls >= MAX_STALLS {
            // Reopened and still nothing: no device takes audio. Paused until Play.
            s.lost = true;
            s.sink.pause();
            let generation = s.generation;
            (self.emit)(generation, Event::PlayState(PlayState::Paused));
            return;
        }
        let stalls = s.stalls + u32::from(stalled);
        self.device_ticks += 1;
        let moved = self.device_ticks >= DEVICE_TICKS && {
            self.device_ticks = 0;
            self.output.moved()
        };
        if stalled || moved {
            return self.reopen(true, stalls);
        }
        self.send_time();
    }

    fn run(&mut self, rx: mpsc::Receiver<Cmd>) {
        let mut last_tick = Instant::now();
        loop {
            // Asleep until a command when nothing plays; otherwise wake for the next tick.
            let cmd = if self.session.as_ref().is_some_and(|s| !s.ended) {
                match rx.recv_timeout(TICK.saturating_sub(last_tick.elapsed())) {
                    Ok(c) => Some(c),
                    Err(mpsc::RecvTimeoutError::Timeout) => None,
                    Err(mpsc::RecvTimeoutError::Disconnected) => return,
                }
            } else {
                match rx.recv() {
                    Ok(c) => Some(c),
                    Err(_) => return,
                }
            };
            if let Some(c) = cmd {
                self.handle(c);
            }
            if last_tick.elapsed() >= TICK {
                last_tick = Instant::now();
                self.tick();
            }
        }
    }
}

/// The player thread.
fn run(rx: mpsc::Receiver<Cmd>, on_event: impl Fn(u64, Event), gain: Gain) {
    let mute = ["UZUMAKI_MUTE", "CADENCE_MUTE"].iter().any(|k| std::env::var(k).is_ok_and(|v| v == "1"));
    // AirPlay: the way to the speaker (None in tests; it carries nothing until one is connected).
    let tap: Option<Arc<dyn CastSink>> = match (crate::airplay::local_feed(), crate::cast::local_feed()) {
        (Some(a), Some(c)) => Some(Arc::new(super::tap::Both(a, c))),
        (Some(a), None) => Some(Arc::new(a)),
        (None, Some(c)) => Some(Arc::new(c)),
        (None, None) => None,
    };
    let mut engine = Engine::new(DeviceOutput::default(), on_event, mute).with_tap(tap);
    engine.tap_gain = gain;
    engine.run(rx);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Sinks without a device; the queue outputs are kept so a test can "play" them.
    #[derive(Clone, Default)]
    struct FakeOutput {
        queues: Arc<Mutex<Vec<rodio::queue::SourcesQueueOutput>>>,
        closed: Arc<AtomicBool>,
        /// The system's default device changed (cleared when a sink is made).
        moved: Arc<AtomicBool>,
        /// No device can be opened.
        none: Arc<AtomicBool>,
    }

    impl Output for FakeOutput {
        fn sink(&mut self) -> Result<Sink, String> {
            if self.none.load(Ordering::SeqCst) {
                return Err("no audio output".into());
            }
            self.moved.store(false, Ordering::SeqCst);
            let (sink, out) = Sink::new();
            self.queues.lock().unwrap().push(out);
            Ok(sink)
        }
        fn close(&mut self) {
            self.closed.store(true, Ordering::SeqCst);
        }
        fn moved(&mut self) -> bool {
            self.moved.load(Ordering::SeqCst)
        }
    }

    type Log = Arc<Mutex<Vec<(u64, Event)>>>;

    fn engine(out: FakeOutput) -> (Engine<FakeOutput, impl Fn(u64, Event)>, Log) {
        let log: Log = Arc::default();
        let l = log.clone();
        (Engine::new(out, move |g, e| l.lock().unwrap().push((g, e)), true), log)
    }

    fn tmp_dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("uzumaki-player-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn crc8(data: &[u8]) -> u8 {
        let mut crc = 0u8;
        for &b in data {
            crc ^= b;
            for _ in 0..8 {
                crc = if crc & 0x80 != 0 { (crc << 1) ^ 0x07 } else { crc << 1 };
            }
        }
        crc
    }

    fn crc16(data: &[u8]) -> u16 {
        let mut crc = 0u16;
        for &b in data {
            crc ^= (b as u16) << 8;
            for _ in 0..8 {
                crc = if crc & 0x8000 != 0 { (crc << 1) ^ 0x8005 } else { crc << 1 };
            }
        }
        crc
    }

    /// `secs` seconds of silence as a mono 16-bit 8 kHz FLAC with verbatim subframes.
    fn flac_bytes(secs: u32) -> Vec<u8> {
        let total = 8000 * secs;
        let mut v = b"fLaC".to_vec();
        v.extend([0x80, 0, 0, 34]); // last metadata block, STREAMINFO
        v.extend(4096u16.to_be_bytes());
        v.extend(4096u16.to_be_bytes());
        v.extend([0u8; 6]); // frame sizes unknown
        // 20 bits rate (8000), 3 bits channels-1, 5 bits bps-1, 36 bits total samples
        let packed: u64 = (8000u64 << 44) | (15u64 << 36) | total as u64;
        v.extend(packed.to_be_bytes());
        v.extend([0u8; 16]); // md5 unset
        let mut left = total;
        let mut frame_no = 0u8;
        while left > 0 {
            let n = left.min(4096);
            left -= n;
            let mut f = vec![0xFF, 0xF8, 0x70, 0x08, frame_no];
            f.extend(((n - 1) as u16).to_be_bytes());
            f.push(crc8(&f));
            f.push(0x02);
            f.extend(std::iter::repeat_n(0u8, n as usize * 2));
            let c = crc16(&f);
            f.extend(c.to_be_bytes());
            v.extend(f);
            frame_no += 1;
        }
        v
    }

    /// `secs` seconds of silence as a mono 16-bit 8 kHz PCM WAV.
    fn pcm_wav(secs: u32) -> Vec<u8> {
        let data = 8000 * secs * 2;
        let mut v = b"RIFF".to_vec();
        v.extend((36 + data).to_le_bytes());
        v.extend(b"WAVEfmt ");
        v.extend(16u32.to_le_bytes());
        v.extend(1u16.to_le_bytes()); // PCM
        v.extend(1u16.to_le_bytes()); // mono
        v.extend(8000u32.to_le_bytes());
        v.extend(16000u32.to_le_bytes()); // byte rate
        v.extend(2u16.to_le_bytes()); // block align
        v.extend(16u16.to_le_bytes()); // bits per sample
        v.extend(b"data");
        v.extend(data.to_le_bytes());
        v.extend(std::iter::repeat_n(0u8, data as usize));
        v
    }

    #[test]
    fn wav_files_decode() {
        let dir = tmp_dir("wav");
        let path = dir.join("a.WAV");
        std::fs::write(&path, pcm_wav(2)).unwrap();
        let d = open_decoder(&path).expect("PCM WAV needs rodio's symphonia-pcm feature");
        let secs = d.total_duration().unwrap().as_secs_f64();
        assert!((secs - 2.0).abs() < 0.01, "{secs}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn song(title: &str, duration: f64) -> Song {
        Song {
            id: "file:///x".into(),
            catalog_id: None,
            title: title.into(),
            artist: "A".into(),
            album: "B".into(),
            album_is_fallback: false,
            duration,
            track_number: None,
            disc_number: None,
            explicit: false,
            artwork: None,
            playable: true,
            genre: None,
            year: None,
            album_id: None,
            artists: Vec::new(),
        }
    }

    fn play_cmd(generation: u64, title: &str, duration: f64, path: &Path, start_at: f64) -> Cmd {
        Cmd::Play { generation, song: song(title, duration), path: path.to_path_buf(), start_at, volume: 0.5 }
    }

    fn states(log: &Log) -> Vec<PlayState> {
        log.lock().unwrap().iter().filter_map(|(_, e)| if let Event::PlayState(s) = e { Some(*s) } else { None }).collect()
    }

    #[test]
    fn play_event_order_and_duration_from_decoder() {
        let dir = tmp_dir("order");
        let path = dir.join("a.flac");
        std::fs::write(&path, flac_bytes(2)).unwrap();
        let (mut e, log) = engine(FakeOutput::default());
        e.handle(play_cmd(7, "A", 0.0, &path, 0.0));
        {
            let l = log.lock().unwrap();
            assert!(matches!(l[0].1, Event::NowPlaying(Some(_))));
            assert!(matches!(l[1].1, Event::PlayState(PlayState::Loading)));
            assert!(matches!(l[2].1, Event::PlayState(PlayState::Playing)), "{l:?}");
            let Event::Time { duration, .. } = l[3].1 else { panic!("{:?}", l[3]) };
            assert!((duration - 2.0).abs() < 0.01, "duration {duration}");
            assert!(l.iter().all(|(g, _)| *g == 7));
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn start_at_seeks_and_song_duration_wins() {
        let dir = tmp_dir("startat");
        let path = dir.join("a.flac");
        std::fs::write(&path, flac_bytes(4)).unwrap();
        let (mut e, log) = engine(FakeOutput::default());
        e.handle(play_cmd(1, "A", 100.0, &path, 2.0));
        let l = log.lock().unwrap();
        let Event::Time { current, duration } = l.last().unwrap().1 else { panic!() };
        assert_eq!(duration, 100.0);
        assert!((current - 2.0).abs() < 0.1, "current {current}");
        drop(l);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ends_exactly_once_and_pause_blocks_tick() {
        let dir = tmp_dir("ended");
        let path = dir.join("a.flac");
        std::fs::write(&path, flac_bytes(1)).unwrap();
        let out = FakeOutput::default();
        let (mut e, log) = engine(out.clone());
        e.handle(play_cmd(3, "A", 0.0, &path, 0.0));
        e.tick();
        assert!(!states(&log).contains(&PlayState::Ended));
        e.handle(Cmd::Pause);
        assert_eq!(states(&log).last(), Some(&PlayState::Paused));
        e.handle(Cmd::Resume);
        assert_eq!(states(&log).last(), Some(&PlayState::Playing));
        // "Play" the whole file through the fake device.
        let n = out.queues.lock().unwrap()[0].by_ref().take(9000).count();
        assert_eq!(n, 9000);
        e.tick();
        e.tick();
        e.tick();
        assert_eq!(states(&log).iter().filter(|s| **s == PlayState::Ended).count(), 1);
        e.handle(Cmd::Stop);
        assert!(out.closed.load(Ordering::SeqCst));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn seek_sends_time_immediately() {
        let dir = tmp_dir("seek");
        let path = dir.join("a.flac");
        std::fs::write(&path, flac_bytes(4)).unwrap();
        let out = FakeOutput::default();
        let (mut e, log) = engine(out.clone());
        e.handle(play_cmd(1, "A", 0.0, &path, 0.0));
        // `Sink::try_seek` waits for the audio thread: stand in for it, slowly.
        let mut queue = out.queues.lock().unwrap().remove(0);
        let stop = Arc::new(AtomicBool::new(false));
        let st = stop.clone();
        let feeder = std::thread::spawn(move || {
            while !st.load(Ordering::SeqCst) {
                let _ = queue.next();
                std::thread::sleep(Duration::from_micros(200));
            }
        });
        let before = log.lock().unwrap().len();
        e.tap_gain.hold(true); // a quiet seek (LocalPlayer::seek_quiet sets it before sending)
        e.handle(Cmd::Seek(3.0));
        assert!(!e.tap_gain.held(), "the seek has landed: this computer may fade in");
        stop.store(true, Ordering::SeqCst);
        feeder.join().unwrap();
        let l = log.lock().unwrap();
        assert_eq!(l.len(), before + 1);
        let Event::Time { current, .. } = l.last().unwrap().1 else { panic!() };
        assert!((current - 3.0).abs() < 0.1, "current {current}");
        drop(l);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn replacing_song_uses_new_generation() {
        let dir = tmp_dir("replace");
        let path = dir.join("a.flac");
        std::fs::write(&path, flac_bytes(1)).unwrap();
        let (mut e, log) = engine(FakeOutput::default());
        e.handle(play_cmd(1, "A", 0.0, &path, 0.0));
        e.handle(play_cmd(2, "B", 0.0, &path, 0.0));
        e.tick();
        let l = log.lock().unwrap();
        assert_eq!(l.last().unwrap().0, 2);
        assert_eq!(e.session.as_ref().unwrap().generation, 2);
        drop(l);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn bad_file_sends_error_then_stopped() {
        let dir = tmp_dir("bad");
        let junk = dir.join("junk.mp3");
        std::fs::write(&junk, b"this is not audio at all").unwrap();
        let missing = dir.join("missing.flac");
        for p in [junk, missing] {
            let (mut e, log) = engine(FakeOutput::default());
            e.handle(play_cmd(5, "Title", 0.0, &p, 0.0));
            let l = log.lock().unwrap();
            let n = l.len();
            let Event::Error(msg) = &l[n - 2].1 else { panic!("{l:?}") };
            assert!(msg.starts_with("Couldn't play “Title”: "), "{msg}");
            assert!(matches!(l[n - 1].1, Event::PlayState(PlayState::Stopped)));
            assert!(l.iter().all(|(g, _)| *g == 5));
            drop(l);
            assert!(e.session.is_none());
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `secs` of a 1 kHz sine at half scale, 16-bit stereo 48 kHz WAV (not 44.1 k: the tap resamples).
    fn sine_wav(secs: u32) -> Vec<u8> {
        let frames = 48000 * secs;
        let data = frames * 4;
        let mut v = b"RIFF".to_vec();
        v.extend((36 + data).to_le_bytes());
        v.extend(b"WAVEfmt ");
        v.extend(16u32.to_le_bytes());
        v.extend(1u16.to_le_bytes());
        v.extend(2u16.to_le_bytes());
        v.extend(48000u32.to_le_bytes());
        v.extend((48000u32 * 4).to_le_bytes());
        v.extend(4u16.to_le_bytes());
        v.extend(16u16.to_le_bytes());
        v.extend(b"data");
        v.extend(data.to_le_bytes());
        for i in 0..frames {
            let s = (16384.0 * (2.0 * std::f64::consts::PI * 1000.0 * i as f64 / 48000.0).sin()) as i16;
            v.extend(s.to_le_bytes());
            v.extend(s.to_le_bytes());
        }
        v
    }

    #[test]
    fn airplay_tap_follows_the_song_and_mutes_without_touching_the_volume() {
        use crate::local::tap::tests::Recorder;
        let dir = tmp_dir("tap");
        let path = dir.join("a.wav");
        std::fs::write(&path, sine_wav(1)).unwrap();
        let out = FakeOutput::default();
        let rec = Recorder::casting();
        let log: Log = Arc::default();
        let l = log.clone();
        let mut e = Engine::new(out.clone(), move |g, ev| l.lock().unwrap().push((g, ev)), false).with_tap(Some(rec.clone()));
        assert!(!rec.active.load(Ordering::SeqCst));
        e.handle(play_cmd(1, "A", 0.0, &path, 0.0));
        assert!(rec.active.load(Ordering::SeqCst), "a loaded song feeds the speaker");
        // The user's volume stays on the sink (and in the engine); only the samples go quiet.
        assert!((e.session.as_ref().unwrap().sink.volume() - 0.5).abs() < 1e-6);
        let device: Vec<f32> = out.queues.lock().unwrap()[0].by_ref().take(48000).collect();
        assert!(device.iter().all(|&s| s == 0.0), "this computer is silent while casting");
        let got = rec.samples();
        assert!(got.len() >= 8192 * 2, "{} samples copied", got.len());
        // Half scale × the app's volume 0.5 → RMS ≈ 0.25/√2.
        let rms = (got.iter().map(|&v| (v as f64 / 32768.0).powi(2)).sum::<f64>() / got.len() as f64).sqrt();
        assert!((rms - 0.25 / 2f64.sqrt()).abs() < 0.01, "rms {rms}");
        e.handle(Cmd::Pause);
        assert!(rec.active.load(Ordering::SeqCst), "paused still owns the speaker (silence)");
        e.handle(Cmd::Stop);
        assert!(!rec.active.load(Ordering::SeqCst), "stopped: the bridge feeds the speaker again");
        assert_eq!(e.volume, 0.5);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn without_a_tap_the_sink_gets_the_plain_decoder() {
        let dir = tmp_dir("notap");
        let path = dir.join("a.wav");
        std::fs::write(&path, sine_wav(1)).unwrap();
        let out = FakeOutput::default();
        let (mut e, _log) = engine(out.clone());
        e.mute = false;
        e.handle(play_cmd(1, "A", 0.0, &path, 0.0));
        let device: Vec<f32> = out.queues.lock().unwrap()[0].by_ref().take(9600).collect();
        assert!(device.iter().any(|&s| s.abs() > 0.2), "plays here as before (half scale × volume 0.5)");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The headless AirPlay dry run for local files: the real controller, the stand-in helper
    /// (`scripts/airplay/fake-cliairplay.py`, measures what reaches its stdin) and a fake
    /// sound card pulled at real time. No device is opened, nothing is sent on the network.
    #[test]
    #[cfg(unix)]
    #[ignore = "dry run with python3, ~6 s: cargo test -p uzumaki airplay_dry_run -- --ignored --nocapture"]
    fn airplay_dry_run_local_file_reaches_the_helper_and_this_computer_stays_silent() {
        use std::os::unix::fs::PermissionsExt;
        use uzumaki_airplay::discover::{Device, Service};
        use uzumaki_airplay::{Cast, Config, Connect, Controller, Event};
        let dir = tmp_dir("airplay-dry");
        let journal = dir.join("journal.txt");
        let path = dir.join("sine.wav");
        std::fs::write(&path, sine_wav(5)).unwrap();
        let fake = Path::new(env!("CARGO_MANIFEST_DIR")).join("../scripts/airplay/fake-cliairplay.py");
        let helper = dir.join("cliairplay");
        std::fs::write(&helper, format!("#!/bin/sh\nFAKE_AIRPLAY_JOURNAL='{}' exec python3 '{}' \"$@\"\n", journal.display(), fake.display())).unwrap();
        std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o755)).unwrap();
        let speaker = Device {
            name: "Bedroom".into(),
            ip: "127.0.0.1".parse().unwrap(),
            hostname: "localhost.".into(),
            airplay: Some(Service { instance: "Bedroom".into(), port: 7000, txt: vec![("features".into(), "0x200,0x0".into())] }),
            raop: None,
        };
        let playing = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let on = playing.clone();
        let ctl = Controller::new(Config { helper: Some(helper), dir: dir.join("run"), fake: vec![speaker], ..Config::from_env(1) }, move |e| {
            if matches!(e, Event::State(Cast::On { .. })) {
                on.store(true, Ordering::SeqCst);
            }
        });
        ctl.set_bridge(false); // the local player only: no bridge capture to wait for
        ctl.connect_named("Bedroom", Connect { volume: 30, password: None, auth: None });
        let t = Instant::now();
        while !playing.load(Ordering::SeqCst) {
            assert!(t.elapsed() < Duration::from_secs(10), "the stand-in never played");
            std::thread::sleep(Duration::from_millis(20));
        }
        let out = FakeOutput::default();
        let log: Log = Arc::default();
        let l = log.clone();
        let feed: Arc<dyn CastSink> = Arc::new(ctl.local_feed());
        let mut e = Engine::new(out.clone(), move |g, ev| l.lock().unwrap().push((g, ev)), false).with_tap(Some(feed));
        e.handle(play_cmd(1, "Sine", 0.0, &path, 0.0));
        // The sound card: 10 ms of 48 kHz stereo every 10 ms, for 4 s.
        let mut queue = out.queues.lock().unwrap().remove(0);
        let card = std::thread::spawn(move || {
            let (mut n, mut peak) = (0usize, 0f32);
            let start = Instant::now();
            for tick in 1..=400u32 {
                for s in queue.by_ref().take(960) {
                    n += 1;
                    peak = peak.max(s.abs());
                }
                if let Some(d) = (start + Duration::from_millis(10 * u64::from(tick))).checked_duration_since(Instant::now()) {
                    std::thread::sleep(d);
                }
            }
            (n, peak)
        });
        let (pulled, peak) = card.join().unwrap();
        e.handle(Cmd::Stop);
        std::thread::sleep(Duration::from_millis(300));
        ctl.shutdown();
        let lines = std::fs::read_to_string(&journal).unwrap_or_default();
        let levels: Vec<&str> = lines.lines().filter(|l| l.starts_with("fake speaker")).collect();
        eprintln!("this computer: {pulled} samples, peak {peak}\nstand-in speaker:\n{}", levels.join("\n"));
        let rms: Vec<f64> = levels.iter().filter_map(|l| l.split("rms ").nth(1)?.split(' ').next()?.parse().ok()).collect();
        assert!(pulled >= 380 * 960, "the fake card ran ({pulled})");
        assert_eq!(peak, 0.0, "no sound out of this computer while casting");
        assert!(rms.len() >= 2, "the helper got audio: {lines}");
        // Half scale × volume 0.5 = 0.25 peak → RMS ≈ 0.177 for full seconds of the sine.
        assert!(rms.iter().any(|&r| r > 0.15), "RMS > 0 at the helper: {rms:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn volume_and_mute() {
        let dir = tmp_dir("vol");
        let path = dir.join("a.flac");
        std::fs::write(&path, flac_bytes(1)).unwrap();
        let log: Log = Arc::default();
        let l = log.clone();
        let mut e = Engine::new(FakeOutput::default(), move |g, ev| l.lock().unwrap().push((g, ev)), false);
        e.handle(play_cmd(1, "A", 0.0, &path, 0.0));
        assert!((e.session.as_ref().unwrap().sink.volume() - 0.5).abs() < 1e-6);
        e.handle(Cmd::Volume(0.25));
        assert!((e.session.as_ref().unwrap().sink.volume() - 0.25).abs() < 1e-6);
        e.mute = true;
        e.handle(Cmd::Volume(0.9));
        assert_eq!(e.session.as_ref().unwrap().sink.volume(), 0.0);
        let _ = std::fs::remove_dir_all(&dir);
    }
    /// Review 2026-11-23 edges L3: the device stops taking audio (unplugged): the position
    /// freezes. The bar must not stay on Playing at a frozen time for good.
    #[test]
    fn review_edges_a_device_that_stops_taking_audio_does_not_stay_playing() {
        let dir = tmp_dir("stall");
        let path = dir.join("a.flac");
        std::fs::write(&path, flac_bytes(4)).unwrap();
        let out = FakeOutput::default();
        let (mut e, log) = engine(out.clone());
        e.handle(play_cmd(1, "A", 0.0, &path, 0.0));
        // Nobody pulls the samples (the device is gone) for 10 s of ticks.
        for _ in 0..40 {
            e.tick();
        }
        let reopened = out.queues.lock().unwrap().len() > 1;
        let last = states(&log).last().copied();
        assert!(reopened || last != Some(PlayState::Playing), "still Playing on a dead device: {:?}", states(&log));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Pulls `n` samples from the newest fake device (it plays them).
    fn pull(out: &FakeOutput, n: usize) {
        let mut q = out.queues.lock().unwrap();
        let last = q.last_mut().unwrap();
        assert_eq!(last.by_ref().take(n).count(), n);
    }

    /// Review 2026-11-23 edges L3: headphones plugged in (the default device changed): the song
    /// moves to them where it was, without a word to the app but the time.
    #[test]
    fn review_edges_the_song_moves_to_the_new_default_device() {
        let dir = tmp_dir("moved");
        let path = dir.join("a.flac");
        std::fs::write(&path, flac_bytes(4)).unwrap();
        let out = FakeOutput::default();
        let (mut e, log) = engine(out.clone());
        e.handle(play_cmd(1, "A", 0.0, &path, 0.0));
        pull(&out, 8000); // 1 s heard
        out.moved.store(true, Ordering::SeqCst);
        for _ in 0..DEVICE_TICKS {
            e.tick();
        }
        assert_eq!(out.queues.lock().unwrap().len(), 2, "a sink on the new device");
        assert!(out.closed.load(Ordering::SeqCst), "the old device was closed");
        assert_eq!(states(&log), [PlayState::Loading, PlayState::Playing], "no state change shown");
        let Event::Time { current, .. } = log.lock().unwrap().last().unwrap().1 else { panic!() };
        assert!((current - 1.0).abs() < 0.1, "carries on at 1 s, not {current}");
        // Play after a pause moves to a device that became the default meanwhile.
        e.handle(Cmd::Pause);
        out.moved.store(true, Ordering::SeqCst);
        e.handle(Cmd::Resume);
        assert_eq!(out.queues.lock().unwrap().len(), 3);
        assert_eq!(states(&log).last(), Some(&PlayState::Playing));
        assert!(!e.session.as_ref().unwrap().sink.is_paused());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// No device takes audio at all: after a few tries the song pauses (the bar says Paused);
    /// Play tries again and carries on where it stopped once a device is back.
    #[test]
    fn review_edges_with_no_device_left_the_song_pauses_and_play_tries_again() {
        let dir = tmp_dir("lost");
        let path = dir.join("a.flac");
        std::fs::write(&path, flac_bytes(4)).unwrap();
        let out = FakeOutput::default();
        let (mut e, log) = engine(out.clone());
        e.handle(play_cmd(1, "A", 0.0, &path, 0.0));
        pull(&out, 8000);
        e.tick();
        out.none.store(true, Ordering::SeqCst);
        for _ in 0..STALL_TICKS {
            e.tick();
        }
        assert_eq!(states(&log).last(), Some(&PlayState::Paused), "{:?}", states(&log));
        // Play with still no device: stays paused.
        e.handle(Cmd::Resume);
        assert_eq!(states(&log).last(), Some(&PlayState::Paused));
        out.none.store(false, Ordering::SeqCst);
        e.handle(Cmd::Resume);
        assert_eq!(states(&log).last(), Some(&PlayState::Playing));
        let Event::Time { current, .. } = log.lock().unwrap().last().unwrap().1 else { panic!() };
        assert!((current - 1.0).abs() < 0.1, "carries on at 1 s, not {current}");
        // A device that opens but never plays: reopened twice, then paused (no endless retries).
        let before = out.queues.lock().unwrap().len();
        for _ in 0..(STALL_TICKS * (MAX_STALLS + 1)) {
            e.tick();
        }
        assert_eq!(out.queues.lock().unwrap().len(), before + MAX_STALLS as usize);
        assert_eq!(states(&log).last(), Some(&PlayState::Paused));
        let _ = std::fs::remove_dir_all(&dir);
    }

}
