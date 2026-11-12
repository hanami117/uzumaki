//! The startup intro: Uzumaki's swirl twists in, then the app twists out of its middle.
//!
//! - **Short** (Settings → Appearance → Startup animation, on by default): about 1.2 s at launch.
//! - **Sync**: the first download of the library. It shows at launch when there is no library on
//!   disk (the first start, or the first after clearing the library cache), and right after
//!   signing in. The swirl keeps turning over the counts until songs, albums and playlists are
//!   in, then twists out. Esc or Skip leaves early; the download carries on behind. It shows
//!   whatever the setting says, since there is nothing to see until the lists arrive.
//!
//! Motion: the twist, zoom and fades are tweens Rune runs itself (`rotate_motion` and friends),
//! set once per phase, so the view isn't rebuilt per frame; `poll` only steps the phases. While
//! an intro is up, arriving data doesn't call `tx.instant()` (`State::landing`), which would land
//! those tweens at once.
//!
//! The swirl is the built-in background's spiral (`uzumaki_fx::intro_swirl`). A logarithmic
//! spiral looks the same when zoomed and turned together, so growing and turning it reads as
//! falling into it.

use std::f32::consts::TAU;
use std::time::Duration;

use rune::{Anim, AnimExt, Curve, Cursor, Cx, Easing, El, LoopSpec, Pixels, SurfaceFit, Transition, Tx, col, el, icon, row, text};

use super::icons;
use super::local_files::thousands;
use crate::state::State;
use crate::theme::*;

pub struct Intro {
    kind: Kind,
    phase: Phase,
    /// Poll time the phase began; set by the first poll after the phase changed.
    since: Option<f64>,
    /// Poll time the intro began (the sync screen's time limit).
    began: Option<f64>,
    /// Seconds in the start pose before twisting in: none at launch (the window opens on it);
    /// while the app is showing, the time it takes to fold away under the swirl.
    hold: f64,
    swirl: Pixels,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Short,
    Sync,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// The first pose: swirl small, turned back and clear; app folded away and clear.
    Start,
    /// The swirl twists in and keeps turning; the app waits, hidden.
    In,
    /// The swirl twists past and grows out of sight while the app twists out to full size.
    Out,
}

/// The swirl picture's side in pixels; it is drawn at the window's diagonal.
const SWIRL_PX: u32 = 1280;
/// Short intro: how long the swirl twists in before the app twists out.
const SHORT_IN: f64 = 0.4;
/// Sync screen: the least time it stays up, so a quick sync doesn't flash.
const SYNC_MIN: f64 = 1.2;
/// Sync screen: stop waiting after this; the download carries on.
const SYNC_MAX: f64 = 180.0;
/// Folding the app away when an intro starts over it (Settings preview, signing in), and how
/// far into the fold the swirl starts twisting in (they overlap, so there is no dark gap).
const FOLD: f64 = 0.45;
pub(super) const FOLD_HOLD: f64 = 0.2;
/// The twist-out, start to finish.
pub(super) const OUT: f64 = 0.85;
/// The swirl's steady turn (anticlockwise, so the arms flow inward): a long linear tween of
/// `SPIN_TURNS` turns, one every `SPIN_PERIOD` seconds. A sync screen up longer than that
/// (5 minutes) holds still, and has long since given up (`SYNC_MAX`).
const SPIN_TURNS: f32 = 60.0;
const SPIN_PERIOD: f32 = 5.0;
/// The app's folded pose: turned back (radians), small.
const APP_TURN: f32 = 0.5;
const APP_SCALE: f32 = 0.3;

impl Intro {
    pub fn new(kind: Kind) -> Self {
        Self { kind, phase: Phase::Start, since: None, began: None, hold: 0.0, swirl: swirl_pixels() }
    }

    fn go(&mut self, phase: Phase) {
        self.phase = phase;
        self.since = None;
    }
}

/// The swirl picture (about 6.5 MB): build it once per intro or onboarding and drop it after.
pub(super) fn swirl_pixels() -> Pixels {
    let swirl = Pixels::new();
    swirl.set(SWIRL_PX, SWIRL_PX, uzumaki_fx::intro_swirl(SWIRL_PX));
    swirl
}

/// The intro to open the window with: the sync screen when there is no library on disk
/// (`Cache::needs_sync`), else the short one if Settings has it on.
pub fn at_launch(enabled: bool, needs_sync: bool) -> Option<Intro> {
    if needs_sync {
        Some(Intro::new(Kind::Sync))
    } else if enabled {
        Some(Intro::new(Kind::Short))
    } else {
        None
    }
}

/// Signed in with nothing on disk (`State::begin_sync`): show the sync screen, or turn a
/// short intro that is still up into it.
pub fn begin_sync(s: &mut State) {
    match &mut s.ui.intro {
        Some(i) if i.phase != Phase::Out => {
            if i.kind != Kind::Sync {
                i.kind = Kind::Sync;
                i.began = None;
            }
        }
        _ => {
            let mut i = Intro::new(Kind::Sync);
            i.hold = FOLD_HOLD;
            s.ui.intro = Some(i);
        }
    }
}

/// The sync screen is up and waiting for the library.
pub fn syncing(s: &State) -> bool {
    s.ui.intro.as_ref().is_some_and(|i| i.kind == Kind::Sync && i.phase != Phase::Out)
}

/// Settings → Appearance → Preview: plays the short intro over the app.
pub fn preview(s: &mut State) {
    if s.ui.intro.is_none() {
        let mut i = Intro::new(Kind::Short);
        i.hold = FOLD_HOLD;
        s.ui.intro = Some(i);
    }
}

/// Esc or Skip: twist out now. A sync carries on in the background.
pub fn skip(s: &mut State) {
    if let Some(i) = &mut s.ui.intro
        && i.phase != Phase::Out
    {
        i.go(Phase::Out);
    }
}

/// Scenes: an intro frozen in `phase` (headless runs have no poll timing to rely on).
pub fn scene(s: &mut State, kind: Kind, phase: Phase) {
    let mut i = Intro::new(kind);
    i.phase = phase;
    i.since = Some(f64::INFINITY); // never "elapsed": poll leaves it alone
    s.ui.intro = Some(i);
}

/// The sync screen can go: the library is in, or there is nothing to wait for (signed out,
/// or the playback engine failed).
fn sync_over(s: &State) -> bool {
    let ses = &s.session;
    ses.error.is_some() || (ses.bridge_up && (!ses.authorized || s.library_synced()))
}

/// `AppSpec::poll`: steps the phases. The motion itself runs in Rune.
pub fn poll(s: &mut State, t: f64, tx: &mut Tx) -> bool {
    let over = s.ui.intro.as_ref().is_some_and(|i| i.kind == Kind::Sync) && sync_over(s);
    let Some(i) = s.ui.intro.as_mut() else { return false };
    let since = *i.since.get_or_insert(t);
    let began = *i.began.get_or_insert(t);
    if since.is_infinite() {
        return false; // a scene
    }
    let elapsed = t - since;
    let (next, wait) = match i.phase {
        // At least one frame in the start pose, so the swirl is mounted before it twists.
        Phase::Start if t > since && elapsed >= i.hold => (Some(Phase::In), 0.0),
        Phase::Start => (None, (i.hold - elapsed).max(0.0)),
        Phase::In => {
            let due = match i.kind {
                Kind::Short => elapsed >= SHORT_IN,
                Kind::Sync => (elapsed >= SYNC_MIN && over) || t - began >= SYNC_MAX,
            };
            let wait = match i.kind {
                Kind::Short => SHORT_IN - elapsed,
                Kind::Sync => (SYNC_MIN - elapsed).max(0.25), // and every page wakes it too
            };
            (due.then_some(Phase::Out), wait)
        }
        Phase::Out if elapsed >= OUT => {
            s.ui.intro = None;
            return true;
        }
        Phase::Out => (None, OUT - elapsed),
    };
    match next {
        Some(p) => {
            i.go(p);
            tx.animated();
            tx.wake_after(Duration::ZERO);
            true
        }
        None => {
            tx.wake_after(Duration::from_secs_f64(wait.max(0.0)));
            false
        }
    }
}

/// The intro to draw. Reduce Motion skips the short one (the sync screen still shows, still).
fn shown<'a>(s: &'a State, cx: &Cx<State>) -> Option<&'a Intro> {
    s.ui.intro.as_ref().filter(|i| i.kind == Kind::Sync || !cx.is_reduced_motion())
}

pub(super) fn tween(secs: f32, easing: Easing) -> Anim {
    Curve::tween(secs, easing).into()
}

/// The app's root, folded away behind the intro and twisting out at the end.
pub fn app(s: &State, cx: &Cx<State>, root: El) -> El {
    let Some(i) = shown(s, cx) else { return root };
    fold(root, i.phase == Phase::Out, false)
}

/// The app's pose behind a swirl layer (the intro's, the onboarding's): folded away (turned
/// back, small and clear), or twisting out to full size (`out`). Reduce Motion: no turn or
/// zoom, only a short fade.
pub(super) fn fold(root: El, out: bool, reduced: bool) -> El {
    if reduced {
        let root = root.rotate(0.0).scale(1.0).rotate_motion(Anim::INSTANT).scale_motion(Anim::INSTANT);
        return if out {
            root.opacity(1.0).opacity_motion(tween(0.2, Easing::EASE_OUT))
        } else {
            root.opacity(0.0).opacity_motion(tween(0.2, Easing::EASE_IN))
        };
    }
    if out {
        root.rotate(0.0)
            .scale(1.0)
            .opacity(1.0)
            .rotate_motion(tween(0.75, Easing::ENTER).delay(0.05))
            .scale_motion(tween(0.75, Easing::ENTER).delay(0.05))
            .opacity_motion(tween(0.3, Easing::EASE_OUT).delay(0.08))
    } else {
        root.rotate(APP_TURN)
            .scale(APP_SCALE)
            .opacity(0.0)
            .rotate_motion(tween(FOLD as f32, Easing::EXIT))
            .scale_motion(tween(FOLD as f32, Easing::EXIT))
            .opacity_motion(tween(FOLD as f32 * 0.8, Easing::EASE_IN))
    }
}

/// The swirl in `phase`'s pose, drawn at the window's diagonal so it covers the window at any
/// turn: `turn` is added to its resting angle (the onboarding turns it a little per step).
/// Reduce Motion: still and upright; it only fades.
pub(super) fn swirl(keys: [&'static str; 2], pixels: &Pixels, win: [f32; 2], phase: Phase, turn: f32, reduced: bool) -> El {
    let d = win[0].hypot(win[1]).ceil();
    let spin = el().key(keys[1]).size(d, d).pixels_fit(pixels, SurfaceFit::Fill);
    let at = (win[0] - d) / 2.0;
    let at_y = (win[1] - d) / 2.0;
    if reduced {
        let alpha = if phase == Phase::In { 1.0 } else { 0.0 };
        return el().key(keys[0]).size(d, d).absolute(at, at_y).opacity(alpha)
            .opacity_motion(tween(0.25, Easing::EASE_OUT))
            .child(spin);
    }
    let spin = match phase {
        Phase::Start => spin.rotate(0.0).rotate_motion(Anim::INSTANT),
        _ => spin.rotate(-TAU * SPIN_TURNS).rotate_motion(Curve::linear(SPIN_TURNS * SPIN_PERIOD)),
    };
    let (rest, scale, alpha, motion, fade) = match phase {
        Phase::Start => (2.4, 0.35, 0.0, Anim::INSTANT, Anim::INSTANT),
        Phase::In => (0.0, 1.0, 1.0, tween(0.8, Easing::ENTER), tween(0.35, Easing::EASE_OUT)),
        Phase::Out => (-2.6, 6.0, 0.0, tween(0.75, Easing::EASE_IN), tween(0.4, Easing::EASE_IN).delay(0.3)),
    };
    el().key(keys[0]).size(d, d).absolute(at, at_y)
        .rotate(rest + turn).scale(scale).opacity(alpha)
        .rotate_motion(motion).scale_motion(motion).opacity_motion(fade)
        .child(spin)
}

/// The intro layer over the whole window: background, swirl and (syncing) the progress.
pub fn overlay(s: &State, cx: &Cx<State>, win: [f32; 2]) -> Option<El> {
    let i = shown(s, cx)?;
    let pin = Some(0.0);
    let bg = el().key("intro/bg").pin(pin, pin, pin, pin).fill(PAL.bg).transition(Transition::opacity());
    let bg = match i.phase {
        Phase::Out => bg.opacity(0.0).opacity_motion(tween(0.45, Easing::EASE_IN_OUT).delay(0.15)),
        _ => bg.opacity(1.0),
    };
    let twist = swirl(["intro/twist", "intro/spin"], &i.swirl, win, i.phase, 0.0, false);

    let mut layer = el().key("intro").pin(pin, pin, pin, pin)
        // Esc skips; the app underneath takes no clicks or keys until the intro is gone.
        .modal(false, skip)
        .on_press(|_: &mut State| {})
        .child(bg)
        .child(twist);
    if i.kind == Kind::Sync {
        layer = layer.children(progress(s, i, pin));
    }
    Some(layer)
}

/// The sync screen's words: the name in the swirl's middle, what is happening, the counts so
/// far, and Skip at the bottom.
fn progress(s: &State, i: &Intro, pin: Option<f32>) -> [El; 2] {
    let ses = &s.session;
    let (title, detail) = if ses.error.is_some() {
        ("Something went wrong", String::new())
    } else if !ses.bridge_up {
        ("Getting ready", "Starting the playback engine\u{2026}".to_string())
    } else if ses.authorized {
        ("Syncing your library", counts(s))
    } else {
        ("Getting ready", String::new())
    };
    let shown = i.phase == Phase::In;
    let fade = |e: El, delay: f32| {
        let e = e.opacity(if shown { 1.0 } else { 0.0 });
        if shown { e.opacity_motion(tween(0.4, Easing::EASE_OUT).delay(delay)) } else { e.opacity_motion(tween(0.15, Easing::EASE_IN)) }
    };

    let mut words = col().gap(6.0).items_center()
        .child(text("Uzumaki").font(HERO).color(PAL.text))
        .child(text(title).font(BODY14).color(PAL.text2));
    if !detail.is_empty() {
        words = words.child(text(detail).font(SMALL12).color(PAL.muted));
    }
    if ses.authorized && let Some(f) = s.sync_fraction() {
        words = words.child(el().h(4.0)).child(bar("intro/bar", 220.0, f, crate::theme::ov(0.12)));
    }
    let middle = fade(col().key("intro/words").pin(pin, pin, pin, pin).center().child(words), 0.25);

    let skip_button = el().key("intro/skip").role("button").label("Skip").focusable(true).h(30.0).px(16.0).radius(15.0)
        .center().fill(crate::theme::ov(0.08)).hover(crate::theme::ov(0.14))
        .press(crate::theme::ov(0.18)).cursor(Cursor::Pointer)
        .on_press(skip)
        .child(text("Skip").font(rune::TextStyle::new(13.0, 600.0)).color(PAL.text2));
    let mut bottom = row().key("intro/bottom").pin(pin, None, pin, Some(36.0)).center().gap(12.0).child(skip_button);
    if ses.bridge_up && ses.authorized {
        bottom = bottom.child(text("Uzumaki keeps syncing in the background").font(SMALL12).color(PAL.dim));
    }
    [middle, fade(bottom, 0.8)]
}

/// "1,234 of 3,312 songs · 210 albums · 35 playlists", counting the lists as their pages
/// arrive ("of" once Apple said the list's length and it isn't all in yet).
pub fn counts(s: &State) -> String {
    let names = [("song", "songs"), ("album", "albums"), ("playlist", "playlists")];
    let parts: Vec<String> = s
        .sync_counts()
        .into_iter()
        .zip(names)
        .filter(|((have, total), _)| *have > 0 || total.is_some_and(|t| t > 0))
        .map(|((have, total), (one, many))| {
            let n = total.unwrap_or(have);
            let noun = if n == 1 { one } else { many };
            match total {
                Some(t) if t > have => format!("{} of {} {noun}", thousands(have), thousands(t)),
                _ => format!("{} {noun}", thousands(have)),
            }
        })
        .collect();
    if parts.is_empty() { "Asking Apple Music\u{2026}".to_string() } else { parts.join(" \u{00b7} ") }
}

/// The pill while syncing: the lists' counts, then each playlist's songs and their covers
/// ("5 of 21 playlists · 300 of 1,950 covers"), with how far the current step is.
fn pill_progress(s: &State) -> (String, Option<f32>) {
    if !s.library_synced() {
        return (counts(s), s.sync_fraction());
    }
    let [(lists, lists_of), (covers, covers_of)] = s.sync_rest();
    let mut parts = Vec::new();
    if lists_of > 0 {
        parts.push(format!("{} of {} playlists", thousands(lists), thousands(lists_of)));
    }
    if covers_of > 0 {
        parts.push(format!("{} of {} covers", thousands(covers), thousands(covers_of)));
    }
    let step = |done: usize, of: usize| (of > 0).then(|| done as f32 / of as f32);
    let f = if lists < lists_of { step(lists, lists_of) } else { step(covers, covers_of) };
    let detail = if parts.is_empty() { counts(s) } else { parts.join(" \u{00b7} ") };
    (detail, f.or(Some(1.0)))
}

/// A thin progress bar, `w` wide, filled to `f` (0..1) in the accent.
pub fn bar(key: &'static str, w: f32, f: f32, track: rune::Color) -> El {
    el().key(key).w(w).h(3.0).radius(1.5).fill(track)
        .child(el().w((w * f.clamp(0.0, 1.0)).max(3.0)).h_full().radius(1.5).fill(PAL.accent))
}

/// How long "Library synced" stays after a background sync.
const DONE_SHOW: f64 = 2.5;

/// `AppSpec::poll`: ends a background sync (the library, its playlists' songs and their covers
/// are in, or the account or engine went away) and times out its "Library synced" note.
pub fn poll_sync(s: &mut State, t: f64, tx: &mut Tx) -> bool {
    let mut changed = false;
    if s.syncing {
        let gone = !s.session.authorized || s.session.error.is_some();
        if gone || s.sync_complete() {
            s.syncing = false;
            changed = true;
            // Only a sync that went on behind the app gets its note (the sync screen says it),
            // and only when every list came in (a failed one shows its error on its page).
            let failed = [&s.library_songs.load, &s.library_albums.load, &s.library_playlists.load]
                .iter()
                .any(|l| matches!(l, crate::state::Load::Failed(_)));
            if !gone && !failed && s.ui.intro.is_none() {
                s.ui.sync_done = Some(t);
            }
        }
    }
    if let Some(done) = s.ui.sync_done {
        if t - done >= DONE_SHOW {
            s.ui.sync_done = None;
            changed = true;
        } else {
            tx.wake_after(Duration::from_secs_f64(DONE_SHOW - (t - done)));
        }
    }
    changed
}

/// The pill at the bottom right of the content area while the library downloads behind the
/// app, with what is in so far, then "Library synced" for a moment.
pub fn sync_pill(s: &State) -> Option<El> {
    let running = s.syncing && !syncing(s);
    if !running && s.ui.sync_done.is_none() {
        return None;
    }
    const W: f32 = 300.0;
    let floating = s.settings.player_style == crate::settings::PlayerStyle::Floating && s.playback.song.is_some();
    let bottom = if floating { super::player_bar::FLOAT_STRIP_H + 4.0 } else { 16.0 };
    let (detail, fraction) = if running { pill_progress(s) } else { (String::new(), None) };
    let (glyph, title, detail) = if running {
        let glyph = icon(&icons::SWIRL, 18.0).color(PAL.accent).looping(LoopSpec::rotation(1.6));
        (glyph, "Syncing your library", detail)
    } else {
        let [songs, albums, playlists] = s.sync_counts().map(|(have, _)| thousands(have));
        let detail = format!("{songs} songs \u{00b7} {albums} albums \u{00b7} {playlists} playlists");
        (icon(&icons::CHECK, 18.0).color(PAL.accent), "Library synced", detail)
    };
    let mut words = col().gap(3.0).grow(1.0).min_w(0.0)
        .child(
            row().w_full().gap(8.0)
                .child(text(title).font(rune::TextStyle::new(13.0, 600.0)).color(PAL.text).grow(1.0))
                .children(fraction.map(|f| {
                    text(format!("{}%", (f * 100.0).floor() as u32)).font(SMALL12).color(PAL.muted)
                })),
        )
        .child(text(detail).font(SMALL12).color(PAL.muted).wrap());
    if let Some(f) = fraction {
        words = words.child(el().h(3.0)).child(bar("sync-pill/bar", W - 62.0, f, PAL.track));
    }
    Some(
        row().key("sync-pill").role("status").label(title).pin(None, None, Some(20.0), Some(bottom)).w(W)
            .px(14.0).py(12.0).gap(12.0).radius(14.0).fill(PAL.player).border(1.0, PAL.field_border)
            .shadow(8.0, 28.0, crate::theme::shadow(0.45))
            .transition(Transition::opacity())
            .child(el().size(18.0, 18.0).center().child(glyph))
            .child(words),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::Load;

    fn state_with(kind: Kind) -> State {
        let mut s = State::offline();
        s.ui.intro = Some(Intro::new(kind));
        s
    }

    fn phase(s: &State) -> Option<Phase> {
        s.ui.intro.as_ref().map(|i| i.phase)
    }

    /// Polls at time `t` with a throwaway transaction.
    fn poll_at(s: &mut State, t: f64) -> bool {
        let mut tx = Tx::default();
        poll(s, t, &mut tx)
    }

    #[test]
    fn short_intro_twists_in_then_out_then_goes() {
        let mut s = state_with(Kind::Short);
        assert!(!poll_at(&mut s, 10.0), "the first frame shows the start pose");
        assert!(poll_at(&mut s, 10.016));
        assert_eq!(phase(&s), Some(Phase::In));
        poll_at(&mut s, 10.02);
        assert!(!poll_at(&mut s, 10.2));
        assert!(poll_at(&mut s, 10.02 + SHORT_IN));
        assert_eq!(phase(&s), Some(Phase::Out));
        poll_at(&mut s, 11.0);
        assert!(poll_at(&mut s, 11.01 + OUT));
        assert_eq!(phase(&s), None);
        let total = OUT + SHORT_IN + 0.04;
        assert!(total <= 1.5, "the short intro takes {total} s");
    }

    #[test]
    fn sync_screen_waits_for_the_library() {
        let mut s = state_with(Kind::Sync);
        poll_at(&mut s, 0.0);
        poll_at(&mut s, 0.02);
        assert_eq!(phase(&s), Some(Phase::In));
        poll_at(&mut s, 0.03);
        // The engine is starting: wait.
        poll_at(&mut s, 5.0);
        assert_eq!(phase(&s), Some(Phase::In));
        // Signed in, lists still loading: wait.
        s.session.bridge_up = true;
        s.session.authorized = true;
        s.library_songs.load = Load::Loading;
        s.library_albums.load = Load::Ready(());
        s.library_playlists.load = Load::Ready(());
        poll_at(&mut s, 8.0);
        assert_eq!(phase(&s), Some(Phase::In));
        assert!(syncing(&s));
        s.library_songs.load = Load::Failed("offline".into());
        poll_at(&mut s, 8.5);
        assert_eq!(phase(&s), Some(Phase::Out), "a failed list doesn't keep the screen up");
        assert!(!syncing(&s));
    }

    #[test]
    fn sync_screen_goes_when_signed_out_or_skipped_or_too_long() {
        let mut s = state_with(Kind::Sync);
        poll_at(&mut s, 0.0);
        poll_at(&mut s, 0.02);
        poll_at(&mut s, 0.03);
        s.session.bridge_up = true; // signed out: nothing to sync, show the sign-in banner
        poll_at(&mut s, 2.0);
        assert_eq!(phase(&s), Some(Phase::Out));

        let mut s = state_with(Kind::Sync);
        poll_at(&mut s, 0.0);
        poll_at(&mut s, 0.02);
        skip(&mut s);
        assert_eq!(phase(&s), Some(Phase::Out));

        let mut s = state_with(Kind::Sync);
        poll_at(&mut s, 0.0);
        poll_at(&mut s, 0.02);
        poll_at(&mut s, 0.03);
        poll_at(&mut s, SYNC_MAX + 1.0);
        assert_eq!(phase(&s), Some(Phase::Out));
    }

    #[test]
    fn signing_in_turns_a_short_intro_into_the_sync_screen() {
        let mut s = state_with(Kind::Short);
        begin_sync(&mut s);
        assert_eq!(s.ui.intro.as_ref().map(|i| i.kind), Some(Kind::Sync));
        let mut s = State::offline();
        begin_sync(&mut s);
        assert!(syncing(&s));
    }

    #[test]
    fn a_background_sync_shows_done_briefly_then_goes() {
        let mut s = State::offline();
        s.session.bridge_up = true;
        s.session.authorized = true;
        s.syncing = true;
        s.library_songs.load = Load::Loading;
        s.library_songs.total = Some(300);
        s.library_albums.load = Load::Ready(());
        s.library_playlists.load = Load::Ready(());
        assert!(sync_pill(&s).is_some(), "running behind the app");
        assert_eq!(s.sync_fraction(), Some(0.0));
        let mut tx = Tx::default();
        assert!(!poll_sync(&mut s, 1.0, &mut tx));
        s.library_songs.load = Load::Ready(());
        assert!(poll_sync(&mut s, 2.0, &mut tx));
        assert!(!s.syncing && s.ui.sync_done == Some(2.0) && sync_pill(&s).is_some(), "Library synced");
        assert!(poll_sync(&mut s, 2.0 + DONE_SHOW, &mut tx));
        assert!(sync_pill(&s).is_none());

        // Finished while the sync screen was up: the screen says so, no note.
        s.syncing = true;
        s.ui.intro = Some(Intro::new(Kind::Sync));
        assert!(sync_pill(&s).is_none(), "the sync screen covers it");
        poll_sync(&mut s, 10.0, &mut tx);
        assert!(!s.syncing && s.ui.sync_done.is_none());
    }

    #[test]
    fn launch_choice() {
        assert_eq!(at_launch(true, true).map(|i| i.kind), Some(Kind::Sync));
        assert_eq!(at_launch(false, true).map(|i| i.kind), Some(Kind::Sync), "the sync screen ignores the setting");
        assert_eq!(at_launch(true, false).map(|i| i.kind), Some(Kind::Short));
        assert!(at_launch(false, false).is_none());
    }
}
