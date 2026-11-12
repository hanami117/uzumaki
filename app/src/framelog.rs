//! Opt-in timing log for checking animations on a real screen, where frame pacing against the
//! compositor can't be seen headless. Off unless `UZUMAKI_FRAME_LOG` names a file; off, every
//! call checks one cached flag and returns.
//!
//! `watch` starts a window (Now Playing opening or closing) in which every frame logs how long
//! after the previous one it started; a gap near two refresh intervals is a dropped frame. Other
//! lines say what happened meanwhile (a background picture rendered, landed or was shared).
//!
//! ```text
//! UZUMAKI_FRAME_LOG=/tmp/uzumaki-frames.log uzumaki
//! ```

use std::io::Write;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

struct Log {
    file: std::fs::File,
    start: Instant,
    /// Frames are logged until then.
    until: Option<Instant>,
    last: Option<Instant>,
    /// Frames and gaps over 25 ms in the current window, for its summary line.
    frames: u32,
    long: u32,
    worst: f64,
}

static LOG: OnceLock<Option<Mutex<Log>>> = OnceLock::new();

fn log() -> Option<&'static Mutex<Log>> {
    LOG.get_or_init(|| {
        let path = std::env::var_os("UZUMAKI_FRAME_LOG").filter(|p| !p.is_empty())?;
        let file = std::fs::OpenOptions::new().create(true).append(true).open(path).ok()?;
        Some(Mutex::new(Log { file, start: Instant::now(), until: None, last: None, frames: 0, long: 0, worst: 0.0 }))
    })
    .as_ref()
}

fn line(l: &mut Log, now: Instant, msg: &str) {
    let ms = now.duration_since(l.start).as_secs_f64() * 1000.0;
    let _ = writeln!(l.file, "{ms:10.1} {msg}");
}

/// Logs `msg` (built only when the log is on).
pub fn event(msg: impl FnOnce() -> String) {
    let Some(l) = log() else { return };
    let mut l = l.lock().unwrap_or_else(|e| e.into_inner());
    line(&mut l, Instant::now(), &msg());
}

/// `what` starts now: log every frame for the next `d`.
pub fn watch(what: &str, d: Duration) {
    let Some(l) = log() else { return };
    let mut l = l.lock().unwrap_or_else(|e| e.into_inner());
    let now = Instant::now();
    line(&mut l, now, &format!("--- {what}"));
    l.until = Some(now + d);
    l.last = Some(now);
    (l.frames, l.long, l.worst) = (0, 0, 0.0);
}

/// Before every frame (`ui::poll`); `t` is Rune's frame time in seconds.
pub fn frame(t: f64) {
    let Some(l) = log() else { return };
    let mut l = l.lock().unwrap_or_else(|e| e.into_inner());
    let Some(until) = l.until else { return };
    let now = Instant::now();
    if now > until {
        l.until = None;
        let (n, long, worst) = (l.frames, l.long, l.worst);
        line(&mut l, now, &format!("--- {n} frames, {long} gaps over 25 ms, longest {worst:.1} ms"));
        return;
    }
    let gap = l.last.map_or(0.0, |p| now.duration_since(p).as_secs_f64() * 1000.0);
    l.last = Some(now);
    l.frames += 1;
    l.worst = l.worst.max(gap);
    let mark = if gap > 25.0 {
        l.long += 1;
        "  <-- long"
    } else {
        ""
    };
    line(&mut l, now, &format!("frame +{gap:5.1} ms  t={t:.4}{mark}"));
}
