//! The ffmpeg path (Linux, and the Mac's fallback): a spawned `ffmpeg` decodes the looping
//! clip to raw RGBA on a pipe; a worker thread reads one frame at a time, waits for its time
//! (the clip's rate) and hands it to `rune::Pixels::set`, which wakes only the window showing
//! it. No `poll` involvement, no ring of frames (the spike measured both).
//!
//! Paused: the worker stops reading, and ffmpeg blocks on the full pipe (no CPU). Dropped:
//! ffmpeg is killed. No ffmpeg installed, or a decode error before the first frame: `Failed`,
//! and the still cover stays.

use std::io::Read;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use super::{Clip, Event, Report};

#[derive(Default)]
struct Flags {
    paused: bool,
    stop: bool,
}

#[derive(Default)]
struct Ctl {
    flags: Mutex<Flags>,
    wake: Condvar,
    child: Mutex<Option<Child>>,
    /// A frame is in `px` (also seen by `poll` where no report can reach the state: headless).
    first: AtomicBool,
}

impl Ctl {
    fn kill(&self) {
        if let Some(mut c) = self.child.lock().unwrap().take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

pub struct Frames {
    px: rune::Pixels,
    ctl: Arc<Ctl>,
}

/// `ffmpeg` on `PATH`, else where Homebrew and most Linux distributions put it (an app started
/// from the Finder or the Dock gets a bare `PATH` without `/opt/homebrew/bin`).
pub fn ffmpeg_path() -> Option<PathBuf> {
    let on_path = std::env::var_os("PATH").into_iter().flat_map(|p| std::env::split_paths(&p).collect::<Vec<_>>());
    let known = ["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin"].into_iter().map(PathBuf::from);
    on_path.chain(known).map(|d| d.join("ffmpeg")).find(|p| p.is_file())
}

impl Frames {
    pub fn start(clip: &Clip, generation: u64, report: Report) -> Self {
        let px = rune::Pixels::new();
        let ctl = Arc::new(Ctl::default());
        let (worker_px, worker_ctl, clip) = (px.clone(), ctl.clone(), clip.clone());
        let spawned = std::thread::Builder::new()
            .name("motion-frames".into())
            .spawn(move || {
                let first = || {
                    worker_ctl.first.store(true, Ordering::Release);
                    report(generation, Event::FirstFrame);
                };
                if let Err(e) = run(&clip, &worker_px, &worker_ctl, &first) {
                    eprintln!("uzumaki: animated artwork: {e}");
                    report(generation, Event::Failed(e));
                }
                worker_ctl.kill();
            });
        if let Err(e) = spawned {
            eprintln!("uzumaki: animated artwork: no worker: {e}");
        }
        Self { px, ctl }
    }

    pub fn pixels(&self) -> &rune::Pixels {
        &self.px
    }

    pub fn has_frame(&self) -> bool {
        self.ctl.first.load(Ordering::Acquire)
    }

    pub fn set_paused(&self, paused: bool) {
        self.ctl.flags.lock().unwrap().paused = paused;
        self.ctl.wake.notify_all();
    }
}

impl Drop for Frames {
    fn drop(&mut self) {
        self.ctl.flags.lock().unwrap().stop = true;
        self.ctl.wake.notify_all();
        // Unblocks a worker waiting on the pipe; it exits on the read error.
        self.ctl.kill();
    }
}

/// Reads and paces frames until stopped. Err only before the first frame is shown (after it,
/// an ending pipe is a stop or ffmpeg dying; the last frame simply stays).
fn run(clip: &Clip, px: &rune::Pixels, ctl: &Ctl, first: &dyn Fn()) -> Result<(), String> {
    let ffmpeg = ffmpeg_path().ok_or("ffmpeg not found (install it for animated artwork)")?;
    let mut cmd = Command::new(ffmpeg);
    cmd.args(["-nostdin", "-v", "error", "-threads", "2"]);
    // The Mac's hardware decoder halves the cost (spike: 2.3 % of a core against 5 %).
    if cfg!(target_os = "macos") {
        cmd.args(["-hwaccel", "videotoolbox"]);
    }
    cmd.args(["-stream_loop", "-1", "-i"]).arg(&clip.path);
    cmd.args(["-an", "-pix_fmt", "rgba", "-f", "rawvideo", "pipe:1"]);
    let mut child = cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().map_err(|e| format!("ffmpeg: {e}"))?;
    let mut out = child.stdout.take().ok_or("ffmpeg: no pipe")?;
    {
        let mut slot = ctl.child.lock().unwrap();
        if ctl.flags.lock().unwrap().stop {
            let _ = child.kill();
            let _ = child.wait();
            return Ok(());
        }
        *slot = Some(child);
    }
    let len = clip.width as usize * clip.height as usize * 4;
    if len == 0 {
        return Err("clip has no size".into());
    }
    let period = Duration::from_secs_f64(1.0 / clip.fps.clamp(1.0, 120.0));
    let mut next = Instant::now();
    let mut shown = false;
    loop {
        {
            let mut f = ctl.flags.lock().unwrap();
            let was_paused = f.paused;
            while f.paused && !f.stop {
                f = ctl.wake.wait(f).unwrap();
            }
            if f.stop {
                return Ok(());
            }
            if was_paused {
                next = Instant::now();
            }
        }
        let mut frame = vec![0u8; len];
        if let Err(e) = out.read_exact(&mut frame) {
            if ctl.flags.lock().unwrap().stop || shown {
                return Ok(());
            }
            return Err(format!("ffmpeg gave no frame: {e}"));
        }
        let now = Instant::now();
        if next > now {
            std::thread::sleep(next - now);
        } else if now - next > period * 12 {
            // Far behind (the machine was busy, or asleep): start counting again from here.
            next = now;
        }
        px.set(clip.width, clip.height, frame);
        if !shown {
            shown = true;
            first();
        }
        next += period;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Clip {
        Clip { path: PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/motion-clip.mp4")), width: 256, height: 256, fps: 24.0 }
    }

    fn wait_for(what: impl Fn() -> bool) -> bool {
        let t = Instant::now();
        while t.elapsed() < Duration::from_secs(10) {
            if what() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        false
    }

    /// The real decoder on the fixture: first frame reported, frames keep coming, pausing holds
    /// them, dropping kills ffmpeg. Skipped where ffmpeg isn't installed.
    #[test]
    fn ffmpeg_plays_pauses_and_stops() {
        if ffmpeg_path().is_none() {
            eprintln!("skipped: no ffmpeg");
            return;
        }
        let events = Arc::new(Mutex::new(Vec::new()));
        let ev = events.clone();
        let frames = Frames::start(&fixture(), 7, Box::new(move |g, e| ev.lock().unwrap().push((g, e))));
        assert!(wait_for(|| !events.lock().unwrap().is_empty()), "no first frame");
        assert_eq!(events.lock().unwrap()[0], (7, Event::FirstFrame));
        let child_pid = frames.ctl.child.lock().unwrap().as_ref().map(|c| c.id()).expect("ffmpeg runs");

        let gen_of = |px: &rune::Pixels| format!("{px:?}");
        let before = gen_of(frames.pixels());
        assert!(wait_for(|| gen_of(frames.pixels()) != before), "frames keep coming");

        frames.set_paused(true);
        std::thread::sleep(Duration::from_millis(150));
        let held = gen_of(frames.pixels());
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(gen_of(frames.pixels()), held, "paused: no new frames");
        frames.set_paused(false);
        assert!(wait_for(|| gen_of(frames.pixels()) != held), "resumed");

        drop(frames);
        // SAFETY: signal 0 only checks whether the process exists.
        assert!(wait_for(|| unsafe { libc::kill(child_pid as i32, 0) } != 0), "ffmpeg is gone after drop");
        assert_eq!(events.lock().unwrap().len(), 1, "no failure on a normal stop");
    }

    #[test]
    fn a_missing_file_fails_before_any_frame() {
        if ffmpeg_path().is_none() {
            return;
        }
        let events = Arc::new(Mutex::new(Vec::new()));
        let ev = events.clone();
        let clip = Clip { path: PathBuf::from("/nonexistent/clip.mp4"), ..fixture() };
        let _frames = Frames::start(&clip, 3, Box::new(move |g, e| ev.lock().unwrap().push((g, e))));
        assert!(wait_for(|| !events.lock().unwrap().is_empty()));
        assert!(matches!(&events.lock().unwrap()[0], (3, Event::Failed(_))));
    }
}
