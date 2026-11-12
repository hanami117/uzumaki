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
//! - `active` is true from `play` until `stop`; `main.rs` reads it on other threads to decide
//!   whether MPRIS follows the bridge or this player.
//! - `UZUMAKI_MUTE=1` (the agents' test switch, as for the bridge; the old `CADENCE_MUTE=1`
//!   still works) forces volume 0.

use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::time::{Duration, Instant};

use rodio::{Decoder, OutputStream, OutputStreamBuilder, Sink, Source};

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
}

impl LocalPlayer {
    /// Starts the thread (it sleeps until the first command). `active` is shared with `main.rs`.
    pub fn spawn(on_event: impl Fn(u64, Event) + Send + 'static, active: Arc<AtomicBool>) -> Self {
        let (tx, rx) = mpsc::channel::<Cmd>();
        std::thread::Builder::new()
            .name("local-player".into())
            .spawn(move || run(rx, on_event))
            .expect("spawn local-player thread");
        Self { tx, active }
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

/// Where sinks come from: the real sound card, or (in tests) a device-less queue.
trait Output {
    /// A fresh sink, opening the device when it isn't open yet.
    fn sink(&mut self) -> Result<Sink, String>;
    /// Closes the device.
    fn close(&mut self);
}

/// The default output device, opened lazily.
#[derive(Default)]
struct DeviceOutput {
    stream: Option<OutputStream>,
}

impl Output for DeviceOutput {
    fn sink(&mut self) -> Result<Sink, String> {
        if self.stream.is_none() {
            let mut stream = OutputStreamBuilder::open_default_stream().map_err(|e| format!("no audio output ({e})"))?;
            stream.log_on_drop(false);
            self.stream = Some(stream);
        }
        Ok(Sink::connect_new(self.stream.as_ref().expect("just opened").mixer()))
    }

    fn close(&mut self) {
        self.stream = None;
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
}

/// The playing logic, independent of the sound card.
struct Engine<O: Output, E: Fn(u64, Event)> {
    output: O,
    emit: E,
    session: Option<Session>,
    volume: f64,
    mute: bool,
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
        Self { output, emit, session: None, volume: 1.0, mute }
    }

    fn gain(&self) -> f32 {
        if self.mute { 0.0 } else { self.volume.clamp(0.0, 1.0) as f32 }
    }

    fn handle(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::Play { generation, song, path, start_at, volume } => self.play(generation, song, &path, start_at, volume),
            Cmd::Pause => {
                if let Some(s) = self.session.as_ref().filter(|s| !s.ended) {
                    s.sink.pause();
                    (self.emit)(s.generation, Event::PlayState(PlayState::Paused));
                }
            }
            Cmd::Resume => {
                if let Some(s) = self.session.as_ref().filter(|s| !s.ended) {
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
                }
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
        sink.append(decoder);
        sink.play();
        Ok(Session { generation, sink, duration, ended: false, offset })
    }

    fn send_time(&self) {
        if let Some(s) = &self.session {
            let current = s.offset + s.sink.get_pos().as_secs_f64();
            let current = if s.duration > 0.0 { current.min(s.duration) } else { current };
            (self.emit)(s.generation, Event::Time { current, duration: s.duration });
        }
    }

    /// Periodic work: the position, and `Ended` (once) when the sink ran dry.
    fn tick(&mut self) {
        let Some(s) = self.session.as_mut() else { return };
        if s.ended || s.sink.is_paused() {
            return;
        }
        if s.sink.empty() {
            s.ended = true;
            let generation = s.generation;
            (self.emit)(generation, Event::PlayState(PlayState::Ended));
        } else {
            self.send_time();
        }
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
fn run(rx: mpsc::Receiver<Cmd>, on_event: impl Fn(u64, Event)) {
    let mute = ["UZUMAKI_MUTE", "CADENCE_MUTE"].iter().any(|k| std::env::var(k).is_ok_and(|v| v == "1"));
    Engine::new(DeviceOutput::default(), on_event, mute).run(rx);
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
    }

    impl Output for FakeOutput {
        fn sink(&mut self) -> Result<Sink, String> {
            let (sink, out) = Sink::new();
            self.queues.lock().unwrap().push(out);
            Ok(sink)
        }
        fn close(&mut self) {
            self.closed.store(true, Ordering::SeqCst);
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
        e.handle(Cmd::Seek(3.0));
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
}
