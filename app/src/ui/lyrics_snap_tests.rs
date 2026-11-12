//! Synced lyrics must glide to every new line, also when a bridge message lands in the same
//! frame. Every bridge message calls `State::landing` (`tx.instant()`), and Rune lands every
//! scroll request and spring of that frame's commit at once, so a line change found in such a
//! frame used to jump instead of glide (live report 2026-11-12, "sometimes the line snaps").
//!
//! The clock is fake (set before every frame); time reports go through `State::apply` as the
//! bridge's do, at irregular intervals, with small backward corrections, a burst, and one on
//! the very frame of each line change.

use std::time::Instant;

use rune::{Env, Runtime};

use crate::bridge::{Event, PlayState};
use crate::state::{Msg, State};

const DT: f64 = 1.0 / 60.0;
/// A glide of one line (~56 pt over 0.75 s) moves at most ~3 pt a frame; a snap moves a whole line.
const MAX_STEP: f32 = 12.0;

/// Runs `scene` for `secs` from `start` on a fake clock and returns the scroll offset of
/// `content` (the scrolled column) per frame, after a settle.
fn run(scene: &str, size: (f32, f32), content: &str, start: f64, secs: f64) -> Vec<f32> {
    let mut s = State::offline();
    (crate::scenes::SCENES.iter().find(|(n, _)| *n == scene).unwrap().1)(&mut s);
    s.playback.state = PlayState::Playing;
    s.playback.position = start;
    s.playback.position_at = Some(Instant::now());
    let line_at = {
        let id = s.playback.song.as_ref().and_then(|x| x.catalog_id.clone()).unwrap();
        let Some(crate::state::Load::Ready(Some(l))) = s.lyrics.get(&id) else { panic!("no lyrics in {scene}") };
        let l = l.clone();
        move |t: f64| l.line_at(t)
    };
    let mut rt = Runtime::new(s, crate::ui::view, size, Env { system_dark: true, scale: 1.0 });
    rt.poll = Some(crate::ui::poll); // as `AppSpec::poll` in the app
    let set_clock = |s: &mut State, t: f64| {
        s.playback.position = t;
        s.playback.position_at = Some(Instant::now());
        s.playback.slew = 0.0;
    };
    // Settle on the start line (placement is an instant jump, as on opening).
    for _ in 0..90 {
        rt.update(|s, _| set_clock(s, start));
        rt.frame(DT);
    }
    let gaps = [13usize, 17, 11, 19, 15, 14, 16];
    let corrections = [0.0, -0.05, 0.02, -0.12, 0.0, -0.03];
    let mut next_report = 5;
    let (mut k, mut burst) = (0usize, 0u32);
    let mut ys = Vec::new();
    let frames = (secs / DT) as usize;
    for f in 0..frames {
        let t = start + f as f64 * DT;
        let crossing = line_at(t) != line_at(t - DT);
        if crossing && k % 3 == 1 {
            burst = 3; // a burst of reports around this line change
        }
        let report = f == next_report || crossing || burst > 0;
        burst = burst.saturating_sub(1);
        if f == next_report {
            next_report += gaps[k % gaps.len()];
            k += 1;
        }
        let corr = corrections[f % corrections.len()];
        rt.update(|s, tx| {
            if report {
                s.apply(Msg::Bridge(Event::Time { current: t + corr, duration: 241.0 }), tx);
            }
            set_clock(s, t);
        });
        rt.frame(DT);
        ys.push(scroll_y(&rt, content));
    }
    ys
}

/// The scroll offset carrying `content` this frame.
fn scroll_y(rt: &Runtime<State>, content: &str) -> f32 {
    let e = rt.elements().into_iter().find(|e| e.key.as_deref() == Some(content) && !e.exiting);
    e.unwrap_or_else(|| panic!("{content} not drawn")).scrolled[1]
}

/// Frames whose scroll moved more than a glide can in one frame: (frame, step).
fn snaps(ys: &[f32]) -> Vec<(usize, f32)> {
    ys.windows(2).enumerate().map(|(i, w)| (i + 1, w[1] - w[0])).filter(|(_, d)| d.abs() > MAX_STEP).collect()
}

fn check(scene: &str, size: (f32, f32), content: &str, start: f64) {
    // Fake lyrics: a line every 6 s; 45 s → 75 s crosses five line changes.
    let ys = run(scene, size, content, start, 30.0);
    // Evidence: `UZUMAKI_LYRICS_TRACE=DIR` writes the per-frame offsets as `DIR/<scene>.csv`.
    if let Some(dir) = std::env::var_os("UZUMAKI_LYRICS_TRACE") {
        let rows: String = ys.iter().enumerate().map(|(i, y)| format!("{i},{:.3},{y:.3}\n", start + i as f64 * DT)).collect();
        std::fs::write(std::path::Path::new(&dir).join(format!("{scene}.csv")), format!("frame,t,scroll_y\n{rows}")).unwrap();
    }
    let moved = (ys[ys.len() - 1] - ys[0]).abs();
    assert!(moved > 150.0, "{scene}: the lyrics should have followed five lines, moved {moved} pt");
    let bad = snaps(&ys);
    assert!(bad.is_empty(), "{scene}: the scroll jumped instead of gliding (frame, pt): {bad:?}");
}

#[test]
fn now_playing_lyrics_glide_to_every_line_while_time_reports_land() {
    check("now-playing", (1180.0, 760.0), "ly-content", 45.0);
}

#[test]
fn right_panel_lyrics_glide_to_every_line_while_time_reports_land() {
    check("right-panel-lyrics", (1360.0, 860.0), "rp-content", 45.0);
}
