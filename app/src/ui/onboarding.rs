//! First-run onboarding. The shell:
//!
//! Welcome → Choice (Apple Music or local files only) → Done, in one full-window layer drawn
//! where the intro is drawn. The app sits folded behind it (`intro::fold`) and twists out of the
//! swirl at the end, as after the intro. Back, Skip (and Esc) and the step dots sit at the bottom.
//! "Show the tour again" (Settings → General) replays Welcome → Tour → Done; the choice and
//! sign-in are not replayed.
//!
//! Local only: no bridge runs while the onboarding is up until Apple Music is chosen, or the
//! tour is skipped (Apple mode is the default). Choosing Apple starts it, and
//! today's sign-in window (`State::show_login`) opens once the app is up. Choosing local files
//! stops it again (after Back) and adds a Music folders step; the app lands on Local Files.
//!
//! Guided sign-in: after choosing Apple Music, the Sign in step explains, then opens
//! the bridge's sign-in window (`signIn`: small, centred; Apple's own page, Uzumaki never sees the
//! password). Its states: getting ready (no `ready` yet), lead-in, waiting, closed without
//! signing in, signed in (the store chip, then on by itself), and the bridge's error. The library
//! syncs behind the rest of the tour (no sync screen; a progress line at the bottom). Settings →
//! Account → Apple Music opens this step on its own (`Mode::SignIn`).
//!
//! Shortcuts tour: after the sign-in or the folders, and on a replay, a tour of the
//! main shortcuts (`ui/tour.rs`): cards from `keys::BINDINGS` with the user's own chords, a
//! small picture of the app that does what each key does, press the key to go on, Next always
//! works. The tour has its own keys (`key`): the layer is a Rune modal, and the app's shortcuts
//! wait while it is up.
//!
//! Motion: the swirl is the intro's (`intro::swirl`), twisted in once, then dimmed behind the
//! words, turning a little further per step. Each step's body is keyed, so Rune slides the old
//! one out and the new one in (mirrored for Back) with no per-frame view runs; `poll` only steps
//! the phases (start pose, Done's pause, the twist-out). Reduce Motion: a still, dim swirl and
//! Rune's short fades.

use std::path::PathBuf;
use std::time::Duration;

use rune::{AnimExt, Cursor, Cx, Easing, El, KeyEvent, LoopSpec, Pixels, Stagger, TextStyle, Transition, Tx, col, el, icon, row, text};

use super::icons;
use super::tour::{self, Tour};
use super::intro::{self, Phase, tween};
use crate::settings::MusicSource;
use crate::state::{Route, State};
use crate::theme::*;

/// The tour's version, saved in `Settings::onboarded` when it is finished or skipped. Raise it
/// only if a later tour should be shown to people who saw this one (nothing does that yet).
pub const TOUR_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Welcome,
    Choice,
    /// Local files only: pick the music folders.
    Folders,
    /// Apple Music: the guided sign-in.
    SignIn,
    /// The shortcuts tour (`ui/tour.rs`).
    Tour,
    Done,
}

/// Where the Sign in step stands, as far as the user's moves decide it. Signed in, a bridge
/// error and "getting ready" come from the session (`sign_in_view`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignIn {
    /// What happens next, and Open Apple sign-in.
    Lead,
    /// Apple's window is open: finish there.
    Waiting,
    /// The user closed Apple's window without signing in.
    Closed,
}

/// What the Sign in step shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SignInView {
    /// The bridge isn't up yet (first starts download Widevine): Open waits.
    Starting,
    Lead,
    Waiting,
    Closed,
    /// Signed in; the store's name or id once known.
    Done(Option<String>),
    Error(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// A new install: Welcome → Choice → Sign in or Folders → Tour → Done.
    FirstRun,
    /// Settings → General → Show again: Welcome → Tour → Done (no choice, no sign-in).
    Replay,
    /// Settings → Account → Apple Music while not signed in: the Sign in step on its own.
    SignIn,
}

impl Mode {
    /// The steps, as far as the choice made (`chosen`) decides them.
    fn steps(self, chosen: Option<MusicSource>) -> &'static [Step] {
        match self {
            Mode::FirstRun if chosen == Some(MusicSource::LocalOnly) => &[Step::Welcome, Step::Choice, Step::Folders, Step::Tour, Step::Done],
            // Not chosen yet: as many steps as either way, so the dots don't change count.
            Mode::FirstRun => &[Step::Welcome, Step::Choice, Step::SignIn, Step::Tour, Step::Done],
            Mode::Replay => &[Step::Welcome, Step::Tour, Step::Done],
            Mode::SignIn => &[Step::SignIn],
        }
    }
}

pub struct Onboarding {
    mode: Mode,
    step: Step,
    /// The last move went back: step bodies slide the other way.
    back: bool,
    phase: Phase,
    /// Poll time the phase (or, in `In`, the Done step) began; set by the first poll after.
    since: Option<f64>,
    /// Seconds in the start pose before twisting in: none at launch; while the app shows (a
    /// replay), the time it takes to fold away.
    hold: f64,
    /// The choice made on this run (`None`: not chosen, or skipped).
    chosen: Option<MusicSource>,
    /// The music folder to offer (`music_dir`), looked up once.
    suggest: Option<PathBuf>,
    /// The suggestion was added by the Folders step itself (pre-ticked), so Back takes it out.
    auto_added: Option<PathBuf>,
    /// The Sign in step's own state.
    sign_in: SignIn,
    /// Poll time the Sign in step saw the sign-in; it moves on `SIGNED_HOLD` later.
    signed_at: Option<f64>,
    /// Our window's frame on screen (points; macOS), to centre Apple's window on it.
    near: Option<[f32; 4]>,
    /// The shortcuts tour's card and its small picture.
    tour: Tour,
    swirl: Pixels,
}

/// Done's "You're all set." stays this long before the twist-out.
const DONE_HOLD: f64 = 1.0;
/// The swirl turns this far further on each step (radians): the spiral itself shows progress.
const STEP_TURN: f32 = -0.45;
/// How far a step's body slides in and out.
const SLIDE: f32 = 24.0;
/// Below this width the Choice cards stack.
const STACK_BELOW: f32 = 720.0;
/// "Signed in" stays this long before the tour goes on by itself.
const SIGNED_HOLD: f64 = 1.2;

impl Onboarding {
    pub fn new(mode: Mode) -> Self {
        Self {
            mode,
            step: Step::Welcome,
            back: false,
            phase: Phase::Start,
            since: None,
            hold: 0.0,
            chosen: None,
            suggest: None,
            auto_added: None,
            sign_in: SignIn::Lead,
            signed_at: None,
            near: None,
            tour: Tour::default(),
            swirl: intro::swirl_pixels(),
        }
    }

    fn steps(&self) -> &'static [Step] {
        self.mode.steps(self.chosen)
    }

    fn index(&self) -> usize {
        self.steps().iter().position(|s| *s == self.step).unwrap_or(0)
    }

    fn go(&mut self, step: Step, back: bool) {
        self.step = step;
        self.back = back;
        if step == Step::Done {
            self.since = None; // Done's pause starts at the next poll
        }
        if step == Step::SignIn {
            self.sign_in = SignIn::Lead;
            self.signed_at = None;
        }
    }
}

/// Whether a launch shows the onboarding: never finished or
/// skipped, and nothing on disk from an earlier run (`Cache::fresh`), and not a second test copy.
/// `forced` (`UZUMAKI_ONBOARDING=1`) shows it anyway, for a look on a scratch config. A
/// `--hidden` start still sets it up: it shows when the window first opens.
pub fn at_launch(onboarded: Option<u32>, fresh_cache: bool, test_instance: bool, forced: bool) -> bool {
    forced || (onboarded.is_none() && fresh_cache && !test_instance)
}

/// Starts the onboarding at launch (the window opens on its start pose). No bridge runs yet.
pub fn start(s: &mut State) {
    let mut o = Onboarding::new(Mode::FirstRun);
    o.suggest = music_dir();
    s.ui.onboarding = Some(o);
    s.ui.intro = None;
}

/// What a launch starts: the onboarding (no bridge until Apple Music is chosen or the tour is
/// skipped), else the intro (no sync screen in local only mode) and, for Apple Music, the
/// bridge (`show_login`: with its sign-in window showing, for `--login`). `hidden`: a `--hidden`
/// start, which plays no intro; the onboarding still waits for the window.
pub fn launch(s: &mut State, onboarding: bool, hidden: bool, needs_sync: bool, show_login: bool) {
    if onboarding {
        start(s);
        return;
    }
    if !hidden {
        s.ui.intro = intro::at_launch(s.settings.startup_animation, needs_sync && !s.local_only());
    }
    s.start_bridge_with(show_login);
}

/// The usual music folder, if it exists: `XDG_MUSIC_DIR` (from `user-dirs.dirs`) on Linux,
/// else `~/Music`. None in tests (they don't look at the real home folder).
fn music_dir() -> Option<PathBuf> {
    if cfg!(test) {
        return None;
    }
    let home = PathBuf::from(std::env::var_os("HOME")?);
    let xdg = if cfg!(target_os = "linux") {
        let config = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".config"));
        std::fs::read_to_string(config.join("user-dirs.dirs")).ok().and_then(|t| xdg_music_dir(&t, &home))
    } else {
        None
    };
    let dir = xdg.unwrap_or_else(|| home.join("Music"));
    // `XDG_MUSIC_DIR="$HOME"` means "no music folder" (xdg-user-dirs' way of turning one off).
    (dir != home && dir.is_dir()).then_some(dir)
}

/// `XDG_MUSIC_DIR="$HOME/Music"` from a `user-dirs.dirs` file.
fn xdg_music_dir(text: &str, home: &std::path::Path) -> Option<PathBuf> {
    let line = text.lines().map(str::trim).find(|l| l.starts_with("XDG_MUSIC_DIR="))?;
    let value = line["XDG_MUSIC_DIR=".len()..].trim().trim_matches('"');
    let path = match value.strip_prefix("$HOME") {
        Some(rest) => home.join(rest.trim_start_matches('/')),
        None if value.starts_with('/') => PathBuf::from(value),
        None => return None,
    };
    Some(path)
}

/// Settings → General → Show again: Welcome → Tour → Done over the app, which folds away first.
pub fn replay(s: &mut State) {
    super::settings::close(s);
    if s.ui.onboarding.is_some() {
        return;
    }
    let mut o = Onboarding::new(Mode::Replay);
    o.hold = intro::FOLD_HOLD;
    s.ui.onboarding = Some(o);
    s.ui.intro = None;
}

/// Settings → Account → Apple Music while not signed in: the Sign in step on its own, over the
/// app, which folds away first. Done or Not now twist back out to where the user was.
pub fn sign_in_only(s: &mut State) {
    super::settings::close(s);
    if s.ui.onboarding.is_some() {
        return;
    }
    let mut o = Onboarding::new(Mode::SignIn);
    o.step = Step::SignIn;
    o.hold = intro::FOLD_HOLD;
    s.ui.onboarding = Some(o);
    s.ui.intro = None;
}

/// The onboarding is up (also while it twists out).
pub fn active(s: &State) -> bool {
    s.ui.onboarding.is_some()
}

/// Records it as seen, so it never shows again by itself.
fn mark_done(s: &mut State) {
    if s.settings.onboarded != Some(TOUR_VERSION) {
        s.settings.onboarded = Some(TOUR_VERSION);
        s.save_settings();
    }
}

/// Get started / Continue: the next step.
pub fn next(s: &mut State) {
    let cards = tour::cards(s);
    let Some(o) = s.ui.onboarding.as_mut() else { return };
    if o.phase != Phase::In {
        return;
    }
    // The tour: the next card, then on.
    if o.step == Step::Tour && o.tour.card + 1 < cards.len() {
        let i = o.tour.card + 1;
        o.tour = Tour::at(i, cards[i]);
        o.back = false;
        return;
    }
    let steps = o.steps();
    let from = o.step;
    let Some(&to) = steps.get(o.index() + 1) else { return };
    o.go(to, false);
    if to == Step::Tour {
        o.tour = Tour::at(0, cards[0]);
    }
    if from == Step::SignIn {
        hide_apple_window(s);
    }
    if to == Step::Done {
        mark_done(s);
    }
    if to == Step::Folders {
        offer_music_dir(s);
    }
}

/// Entering the Folders step with no folders yet: the usual music folder goes in pre-ticked, and
/// its scan starts at once, so the library is there when the tour ends.
fn offer_music_dir(s: &mut State) {
    let Some(o) = s.ui.onboarding.as_mut() else { return };
    let Some(dir) = o.suggest.clone() else { return };
    if !s.settings.music_folders.is_empty() {
        return;
    }
    o.auto_added = Some(dir.clone());
    s.add_music_folder(dir);
}

/// Folders step: the suggested folder, ticked or not.
fn toggle_suggested(s: &mut State) {
    let Some(dir) = s.ui.onboarding.as_ref().and_then(|o| o.suggest.clone()) else { return };
    match s.settings.music_folders.iter().position(|f| *f == dir) {
        Some(i) => s.remove_music_folder(i),
        None => s.add_music_folder(dir),
    }
    if let Some(o) = s.ui.onboarding.as_mut() {
        o.auto_added = None; // the user's own choice now
    }
}

/// Back: the step before (none from Welcome or Done).
pub fn back(s: &mut State) {
    let cards = tour::cards(s);
    let authorized = s.session.authorized;
    let Some(o) = s.ui.onboarding.as_mut() else { return };
    if o.phase != Phase::In || o.step == Step::Done || (o.step == Step::SignIn && authorized) {
        return;
    }
    // The tour: the card before, then the step before.
    if o.step == Step::Tour && o.tour.card > 0 {
        let i = o.tour.card.min(cards.len()) - 1;
        o.tour = Tour::at(i, cards[i]);
        o.back = true;
        return;
    }
    if !can_go_back(o, authorized) {
        return;
    }
    let i = o.index();
    if i == 0 {
        return;
    }
    let to = o.steps()[i - 1];
    let from = o.step;
    // Leaving Folders for the Choice: the folder it added by itself goes again.
    let undo = if o.step == Step::Folders { o.auto_added.take() } else { None };
    o.go(to, true);
    if from == Step::SignIn {
        hide_apple_window(s);
    }
    if let Some(i) = undo.and_then(|d| s.settings.music_folders.iter().position(|f| *f == d)) {
        s.remove_music_folder(i);
    }
}

/// Whether Back has somewhere to go: not from Welcome or Done, and not from the tour's first
/// card into a sign-in that already happened (it would only move on again by itself).
fn can_go_back(o: &Onboarding, authorized: bool) -> bool {
    let i = o.index();
    if i == 0 || o.step == Step::Done {
        return false;
    }
    let before = o.steps()[i - 1];
    !(o.step == Step::SignIn && authorized) && !(o.step == Step::Tour && o.tour.card == 0 && before == Step::SignIn && authorized)
}

/// A key while the onboarding is up (it is a Rune modal, so the app's shortcuts wait). On the
/// tour, the card's shortcut, as the user has it now, tries the card: the picture does what the
/// app would (and again, as often as the user likes), the keycap flashes, and the card stays
/// until Next or Return. Shortcuts of
/// other cards are swallowed (Space mustn't press a focused button there); every other key
/// (Return, Esc, Tab) goes on to Rune: Next, Skip, focus.
pub fn key(s: &mut State, ev: &KeyEvent, tx: &mut Tx) -> bool {
    let Some(o) = s.ui.onboarding.as_ref() else { return false };
    if o.phase != Phase::In || o.step != Step::Tour {
        return false;
    }
    let Some(action) = super::keys::action_for(s, ev, tx) else { return false };
    let local = s.local_only();
    let cards = tour::cards(s);
    let Some(o) = s.ui.onboarding.as_mut() else { return false };
    let Some(&card) = cards.get(o.tour.card) else { return false };
    if !card.takes(action, local) {
        return cards.iter().any(|c| c.takes(action, local));
    }
    if tx.key_repeat() {
        return true;
    }
    o.tour.press(action, local);
    tx.wake_after(Duration::from_secs_f64(tour::FLASH));
    true
}

/// The menu bar (macOS): whether the tour's current card takes `action`, so its menu item
/// stays enabled during the onboarding and plays the card's effect in the picture.
#[cfg(any(target_os = "macos", test))]
pub fn menu_takes(s: &State, action: super::keys::Action) -> bool {
    let Some(o) = s.ui.onboarding.as_ref() else { return false };
    if o.phase != Phase::In || o.step != Step::Tour {
        return false;
    }
    tour::cards(s).get(o.tour.card).is_some_and(|c| c.takes(action, s.local_only()))
}

/// A menu bar item picked during the onboarding: the card's effect, as its shortcut does
/// (`key`); anything else does nothing behind the onboarding.
#[cfg(any(target_os = "macos", test))]
pub fn menu_press(s: &mut State, action: super::keys::Action, tx: &mut Tx) {
    if !menu_takes(s, action) {
        return;
    }
    let local = s.local_only();
    if let Some(o) = s.ui.onboarding.as_mut() {
        o.tour.press(action, local);
        tx.wake_after(Duration::from_secs_f64(tour::FLASH));
    }
}

/// Choice: Apple Music or local files only. Stored at once. Apple starts the bridge now
/// (the sign-in waits for it, and Widevine's first download takes a while); local
/// only stops one started by an earlier Apple choice (after Back).
pub fn choose(s: &mut State, source: MusicSource) {
    let Some(o) = s.ui.onboarding.as_mut() else { return };
    if o.phase != Phase::In || o.step != Step::Choice {
        return;
    }
    o.chosen = Some(source);
    s.set_music_source(source);
    next(s);
}

/// Skip and Esc: straight into the app with the defaults (Apple mode, not signed in: the app's
/// usual sign-in banner). Seen counts as done.
pub fn skip(s: &mut State) {
    let Some(o) = s.ui.onboarding.as_mut() else { return };
    if o.phase == Phase::Out {
        return;
    }
    let mode = o.mode;
    o.phase = Phase::Out;
    o.since = None;
    if o.step == Step::SignIn {
        hide_apple_window(s);
    }
    if mode == Mode::SignIn {
        return; // the choice is made and the bridge runs: just back to the app
    }
    if let Some(o) = s.ui.onboarding.as_mut() {
        o.chosen = None;
    }
    mark_done(s);
    // Apple mode (the default) needs its bridge; local only (chosen before Back) starts none.
    s.start_bridge();
}

/// Apple's window, if this step opened it and the sign-in didn't close it already.
fn hide_apple_window(s: &mut State) {
    if s.session.login_window && !s.session.authorized {
        s.show_login(false);
    }
}

/// What the Sign in step shows now.
pub fn sign_in_view(s: &State, o: &Onboarding) -> SignInView {
    let ses = &s.session;
    if ses.authorized {
        return SignInView::Done(s.settings.storefront.as_ref().map(|f| f.label()));
    }
    if let Some(e) = &ses.error {
        return SignInView::Error(e.clone());
    }
    match o.sign_in {
        // A sign-in window we opened went away without our `hide`: as good as closed.
        SignIn::Waiting if !ses.login_window => SignInView::Closed,
        SignIn::Waiting => SignInView::Waiting,
        SignIn::Closed => SignInView::Closed,
        SignIn::Lead if !ses.bridge_up => SignInView::Starting,
        SignIn::Lead => SignInView::Lead,
    }
}

/// On the Sign in step, not signed in.
fn signing_in(s: &mut State) -> Option<&mut Onboarding> {
    let authorized = s.session.authorized;
    s.ui.onboarding.as_mut().filter(|o| o.phase == Phase::In && o.step == Step::SignIn && !authorized)
}

/// Open Apple sign-in, Bring the window back, Try again: Apple's window, small and centred.
pub fn open_sign_in(s: &mut State) {
    if !s.session.bridge_up || s.session.error.is_some() {
        return; // the button says Getting ready…
    }
    let Some(o) = signing_in(s) else { return };
    o.sign_in = SignIn::Waiting;
    let near = if cfg!(target_os = "macos") { o.near } else { None };
    s.sign_in_window(near);
}

/// Cancel while waiting: Apple's window hides, back to the lead-in.
fn cancel_sign_in(s: &mut State) {
    let Some(o) = signing_in(s) else { return };
    o.sign_in = SignIn::Lead;
    hide_apple_window(s);
}

/// Not now: on without signing in (the app shows its usual sign-in banner). On its own (from
/// Settings), back to the app.
fn not_now(s: &mut State) {
    let Some(o) = s.ui.onboarding.as_ref().filter(|o| o.phase == Phase::In && o.step == Step::SignIn) else { return };
    if o.mode == Mode::SignIn {
        skip(s);
    } else {
        next(s);
    }
}

/// The bridge reported its window closed by the user: if this step was waiting on it, say so.
pub fn window_closed(s: &mut State) {
    if let Some(o) = signing_in(s).filter(|o| o.sign_in == SignIn::Waiting) {
        o.sign_in = SignIn::Closed;
    }
}

/// The bridge failed: Try again starts a new one.
fn retry_bridge(s: &mut State) {
    let Some(o) = signing_in(s) else { return };
    o.sign_in = SignIn::Lead;
    s.restart_bridge();
}

/// The bridge failed: local files only instead (first run: on to the Folders step).
fn use_local(s: &mut State) {
    let Some(o) = signing_in(s) else { return };
    if o.mode == Mode::SignIn {
        s.set_music_source(MusicSource::LocalOnly);
        skip(s);
        return;
    }
    o.chosen = Some(MusicSource::LocalOnly);
    o.go(Step::Folders, false);
    s.set_music_source(MusicSource::LocalOnly);
    offer_music_dir(s);
}

/// The twist-out begins: land on the start page for the choice. Apple's sign-in had its own
/// step; not signed in, the app shows its usual banner.
fn leave(s: &mut State) {
    let Some(o) = s.ui.onboarding.as_ref() else { return };
    if o.chosen == Some(MusicSource::LocalOnly) {
        s.navigate(Route::LocalFiles);
        s.back_stack.clear(); // the start page, not a page to go back from
    }
}

/// Scenes: frozen on `step` (headless runs have no poll timing to rely on).
pub fn scene(s: &mut State, mode: Mode, step: Step, chosen: Option<MusicSource>) {
    let mut o = Onboarding::new(mode);
    o.step = step;
    o.chosen = chosen;
    o.phase = Phase::In;
    o.since = Some(f64::INFINITY);
    s.ui.onboarding = Some(o);
    s.ui.intro = None;
    if step == Step::Tour {
        scene_tour(s, 0, &[]);
    }
}

/// Scenes: the tour on card `i`, with these of its shortcuts pressed in order (the picture
/// after them): `Action::id`s, or "slot3" for a number key. A trailing "settle" lets the keycap's
/// flash end (the state a moment after the last press).
pub fn scene_tour(s: &mut State, i: usize, pressed: &[&str]) {
    use super::keys::{Action, BINDINGS};
    let cards = tour::cards(s);
    let local = s.local_only();
    if let Some(o) = s.ui.onboarding.as_mut() {
        let i = i.min(cards.len() - 1);
        o.tour = Tour::at(i, cards[i]);
        for id in pressed {
            let action = match id.strip_prefix("slot") {
                Some(n) => n.parse().ok().map(Action::Slot),
                None => BINDINGS.iter().find(|b| b.action.id() == *id).map(|b| b.action),
            };
            match (action, *id) {
                (Some(a), _) => o.tour.press(a, local),
                (None, "settle") => o.tour.lit = false,
                _ => {}
            }
        }
    }
}

/// Scenes: the Sign in step's own state.
pub fn scene_sign_in(s: &mut State, state: SignIn) {
    if let Some(o) = s.ui.onboarding.as_mut() {
        o.sign_in = state;
    }
}

/// Scenes: the music folder the Folders step offers.
pub fn scene_suggest(s: &mut State, dir: PathBuf) {
    if let Some(o) = s.ui.onboarding.as_mut() {
        o.suggest = Some(dir);
    }
}

/// `AppSpec::poll`: steps the phases; the motion itself runs in Rune.
pub fn poll(s: &mut State, t: f64, tx: &mut Tx) -> bool {
    let Some(o) = s.ui.onboarding.as_mut() else { return false };
    let since = *o.since.get_or_insert(t);
    if since.is_infinite() {
        return false; // a scene
    }
    let elapsed = t - since;
    match o.phase {
        // At least one frame in the start pose, so the swirl is mounted before it twists.
        Phase::Start if t > since && elapsed >= o.hold => {
            o.phase = Phase::In;
            o.since = None;
            tx.animated();
            tx.wake_after(Duration::ZERO);
            true
        }
        Phase::Start => {
            tx.wake_after(Duration::from_secs_f64((o.hold - elapsed).max(0.0)));
            false
        }
        // Signed in: "Signed in" for a moment, then on by itself (on its own: back to the app).
        Phase::In if o.step == Step::SignIn && s.session.authorized => {
            let at = *o.signed_at.get_or_insert(t);
            if t - at < SIGNED_HOLD {
                tx.wake_after(Duration::from_secs_f64(SIGNED_HOLD - (t - at)));
                return false;
            }
            if o.mode == Mode::SignIn {
                o.phase = Phase::Out;
                o.since = None;
                tx.animated();
                tx.wake_after(Duration::ZERO);
            } else {
                next(s);
            }
            true
        }
        // A pressed key's cap flashes coral for a moment; the card stays until Next.
        Phase::In if o.step == Step::Tour && o.tour.lit => {
            let at = *o.tour.lit_at.get_or_insert(t);
            if t - at < tour::FLASH {
                tx.wake_after(Duration::from_secs_f64(tour::FLASH - (t - at)));
                return false;
            }
            o.tour.lit = false;
            o.tour.lit_at = None;
            true
        }
        Phase::In if o.step == Step::Done && elapsed >= DONE_HOLD => {
            o.phase = Phase::Out;
            o.since = None;
            leave(s);
            tx.animated();
            tx.wake_after(Duration::ZERO);
            true
        }
        Phase::In if o.step == Step::Done => {
            tx.wake_after(Duration::from_secs_f64(DONE_HOLD - elapsed));
            false
        }
        Phase::In => false, // waits for the user
        Phase::Out if elapsed >= intro::OUT => {
            s.ui.onboarding = None; // drops the swirl picture
            true
        }
        Phase::Out => {
            tx.wake_after(Duration::from_secs_f64(intro::OUT - elapsed));
            false
        }
    }
}

/// The app's root, folded away behind the onboarding and twisting out at the end.
pub fn app(s: &State, cx: &Cx<State>, root: El) -> El {
    let Some(o) = &s.ui.onboarding else { return root };
    intro::fold(root, o.phase == Phase::Out, cx.is_reduced_motion())
}

/// The onboarding layer over the whole window.
pub fn overlay(s: &State, cx: &Cx<State>, win: [f32; 2]) -> Option<El> {
    let o = s.ui.onboarding.as_ref()?;
    let reduced = cx.is_reduced_motion();
    let pin = Some(0.0);

    let bg = el().key("onb/bg").pin(pin, pin, pin, pin).fill(PAL.bg);
    let bg = match o.phase {
        Phase::Out if reduced => bg.opacity(0.0).opacity_motion(tween(0.2, Easing::EASE_IN_OUT)),
        Phase::Out => bg.opacity(0.0).opacity_motion(tween(0.45, Easing::EASE_IN_OUT).delay(0.15)),
        _ => bg.opacity(1.0),
    };

    // The swirl twists in at full strength, then dims behind the words; at the end it comes
    // back up as it twists past.
    let turn = STEP_TURN * o.index() as f32;
    let swirl = intro::swirl(["onb/twist", "onb/spin"], &o.swirl, win, o.phase, turn, reduced);
    let dim = el().key("onb/dim").pin(pin, pin, pin, pin).a11y_hidden().child(swirl);
    let dim = match o.phase {
        _ if reduced => dim.opacity(0.35),
        Phase::Start => dim.opacity(1.0),
        Phase::In => dim.opacity(0.35).opacity_motion(tween(0.7, Easing::EASE_IN_OUT).delay(0.6)),
        Phase::Out => dim.opacity(1.0).opacity_motion(tween(0.3, Easing::EASE_OUT)),
    };

    let mut layer = el().key("onboarding").pin(pin, pin, pin, pin)
        // Esc skips; the app underneath takes no clicks or keys until the onboarding is gone.
        .modal(false, skip)
        .on_press(|_: &mut State| {})
        // Screen readers: a dialog over the app (the click catcher above would make it a button).
        .role("dialog")
        .label(match o.mode {
            Mode::FirstRun => "Welcome to Uzumaki",
            Mode::Replay => "Uzumaki tour",
            Mode::SignIn => "Sign in to Apple Music",
        })
        // The tour's keys while a button in the layer has focus (Rune offers them here first;
        // with nothing focused they come through the app's `on_key`, which sends them to `key`).
        .on_key(key)
        // macOS: where our window is, so Apple's sign-in window opens centred on it.
        .on_screen_layout(|s: &mut State, r: [f32; 4]| {
            if let Some(o) = s.ui.onboarding.as_mut() {
                o.near = Some(r);
            }
        })
        .child(bg)
        .child(dim);
    if o.phase != Phase::Start {
        layer = layer.child(body(s, cx, o, win)).children(sync_line(s, win)).child(bottom(s, o));
    }
    Some(layer)
}

/// The step's words and buttons, centred. Keyed per step: the old one leaves the way the new
/// one enters (`child_exit`), mirrored for Back.
fn body(s: &State, cx: &Cx<State>, o: &Onboarding, win: [f32; 2]) -> El {
    let sign = if o.back { -1.0 } else { 1.0 };
    let enter = Transition::offset(SLIDE * sign, 0.0).combined(Transition::opacity());
    let exit = Transition::offset(-SLIDE * sign, 0.0).combined(Transition::opacity());
    let pin = Some(0.0);
    let content = match o.step {
        Step::Tour => tour::view(s, &o.tour, win, o.back, cx.is_reduced_motion(), primary("onb/tour/next", if o.tour.card + 1 >= tour::cards(s).len() { "Finish" } else { "Next" }, next)),
        Step::Welcome => welcome(),
        Step::Choice => choice(o, win),
        Step::Folders => folders(s, o, win),
        Step::SignIn => sign_in(s, o, win),
        Step::Done => done(s, o),
    };
    let key = match o.step {
        Step::Welcome => "onb/step/welcome",
        Step::Choice => "onb/step/choice",
        Step::Folders => "onb/step/folders",
        Step::SignIn => "onb/step/signin",
        Step::Tour => "onb/step/tour",
        Step::Done => "onb/step/done",
    };
    // Pinned, so a leaving step and the arriving one overlap instead of stacking.
    let step = col().key(key).pin(pin, pin, pin, pin).center().px(32.0).transition(enter).child(content);
    let shown = o.phase == Phase::In;
    col().key("onb/body").pin(pin, pin, pin, Some(84.0)).center()
        .child_exit(exit)
        .opacity(if shown { 1.0 } else { 0.0 })
        .opacity_motion(tween(if shown { 0.3 } else { 0.2 }, Easing::EASE_OUT))
        .child(step)
}

/// Rise-and-fade entrances for a step's parts, 0.06 s apart.
fn staggered(parts: Vec<El>) -> Vec<El> {
    let st = Stagger::new(0.06);
    let n = parts.len();
    parts
        .into_iter()
        .enumerate()
        .map(|(i, e)| e.transition(Transition::offset(0.0, 12.0).combined(Transition::opacity())).enter_delay(st.delay(i, n)))
        .collect()
}

/// The coral pill: the step's main button, which Return presses.
fn primary(key: &'static str, label: &'static str, press: fn(&mut State)) -> El {
    el().key(key).role("button").label(label).focusable(true).default_button().h(44.0).px(26.0).radius(22.0).center()
        .fill(PAL.accent).hover(ov(0.08)).press(rune::Color::BLACK.alpha(0.12)).cursor(Cursor::Pointer)
        .on_press(press)
        .child(text(label).font(BUTTON).color(PAL.ink))
}

/// A quiet text button for the bottom bar.
fn quiet(key: &'static str, label: &'static str, press: fn(&mut State)) -> El {
    el().key(key).role("button").label(label).focusable(true).h(30.0).px(16.0).radius(15.0).center()
        .fill(ov(0.06)).hover(ov(0.12)).press(ov(0.16)).cursor(Cursor::Pointer)
        .on_press(press)
        .child(text(label).font(TextStyle::new(13.0, 600.0)).color(PAL.text2))
}

const TAGLINE: &str = if cfg!(target_os = "macos") {
    "Alternative native frontend for Apple Music. For your Mac."
} else if cfg!(target_os = "linux") {
    "Alternative native frontend for Apple Music. For Linux."
} else {
    "Alternative native frontend for Apple Music."
};

fn welcome() -> El {
    col().items_center().gap(0.0).children(staggered(vec![
        text("Uzumaki").key("onb/welcome/title").font(TextStyle::new(52.0, 600.0).tracking(-1.3)).color(PAL.text),
        text(TAGLINE).key("onb/welcome/line").font(TextStyle::new(17.0, 400.0)).color(PAL.text2).mt(10.0),
        el().key("onb/welcome/go").mt(34.0).child(primary("onb/start", "Get started", next)),
    ]))
}

fn choice(o: &Onboarding, win: [f32; 2]) -> El {
    let stacked = win[0] < STACK_BELOW;
    // The card chosen earlier on this run (after Back) keeps its ring.
    let picked = o.chosen;
    let apple = card(
        "onb/choice/apple",
        &icons::SONGS,
        "Apple Music",
        "Stream your Apple Music library. Your music folders work too.",
        Some("Needs an Apple Music subscription."),
        picked == Some(MusicSource::Apple),
        stacked,
    )
    .on_press(|s: &mut State| choose(s, MusicSource::Apple));
    let local = card(
        "onb/choice/local",
        &icons::LOCAL_FILES,
        "Local files only",
        "Play music from folders on this computer. No Apple account, nothing runs in the background.",
        None,
        picked == Some(MusicSource::LocalOnly),
        stacked,
    )
    .on_press(|s: &mut State| choose(s, MusicSource::LocalOnly));
    let cards = staggered(vec![apple, local]);
    let cards = if stacked { col().key("onb/cards").gap(12.0).children(cards) } else { row().key("onb/cards").gap(16.0).children(cards) };
    col().items_center().children(staggered(vec![
        text("Where does your music come from?").key("onb/choice/title").font(H1).color(PAL.text),
        text("Pick one to start. Uzumaki can do both.").key("onb/choice/sub").font(BODY14).color(PAL.text2).mt(8.0),
    ]))
    .child(el().h(if stacked { 20.0 } else { 30.0 }))
    .child(cards)
    .child(
        text("You can change this later in Settings \u{2192} Account.").key("onb/choice/note").font(SMALL13).color(PAL.dim).mt(18.0)
            .transition(Transition::opacity())
            .enter_delay(0.2),
    )
}

fn card(key: &'static str, ic: &'static rune::Icon, title: &'static str, desc: &'static str, extra: Option<&'static str>, on: bool, stacked: bool) -> El {
    let w = if stacked { 340.0 } else { 290.0 };
    let ring = if on { PAL.accent } else { PAL.field_border };
    let badge = el().size(40.0, 40.0).radius(12.0).center().fill(PAL.accent.alpha(0.16)).child(icon(ic, 20.0).color(PAL.accent));
    let mut words = col().gap(6.0).min_w(0.0).grow(1.0).w_full()
        .child(text(title).font(TextStyle::new(17.0, 600.0)).color(PAL.text))
        .child(text(desc).font(SMALL13).color(PAL.text2).wrap());
    if let Some(x) = extra {
        words = words.child(text(x).font(SMALL12).color(PAL.dim).wrap().mt(2.0));
    }
    let inner = if stacked {
        row().w_full().gap(16.0).items_start().child(badge).child(words)
    } else {
        col().w_full().gap(16.0).child(badge).child(words)
    };
    let card = el().key(key).role("button").label(title).focusable(true).w(w).padding(if stacked { 18.0 } else { 22.0 })
        .radius(18.0).fill(PAL.player).border(if on { 2.0 } else { 1.0 }, ring)
        .hover(ov(0.04)).press(ov(0.08)).cursor(Cursor::Pointer)
        .shadow(6.0, 24.0, shadow(0.25))
        .child(inner);
    if stacked { card } else { card.h(200.0) }
}

/// Folders: the music folders, the suggested one pre-ticked, the count as the scan grows (one
/// view run per scan batch), Add folder… (the native picker) and Continue. Continue works with
/// no folders too: Local Files then shows its empty state.
fn folders(s: &State, o: &Onboarding, win: [f32; 2]) -> El {
    let w = (win[0] - 64.0).clamp(280.0, 460.0);
    let lib = &s.local_lib;
    let list = &s.settings.music_folders;
    let enter = || Transition::offset(0.0, 10.0).combined(Transition::opacity());
    let mut rows = col().key("onb/folders/list").w(w).gap(8.0);
    // The suggestion, while it isn't ticked: a row to tick it again.
    if let Some(dir) = o.suggest.as_ref().filter(|d| !list.contains(d)) {
        rows = rows.child(
            folder_row("onb/folders/suggested".into(), dir, false)
                .on_press(toggle_suggested)
                .transition(enter()),
        );
    }
    for (i, f) in list.iter().enumerate() {
        let suggested = o.suggest.as_ref() == Some(f);
        let row = folder_row(format!("onb/folder/{}", f.display()), f, true);
        let row = if suggested {
            row.on_press(toggle_suggested)
        } else {
            row.on_press(move |s: &mut State| s.remove_music_folder(i))
        };
        rows = rows.child(row.transition(enter()));
    }
    let count = if list.is_empty() {
        "No folders yet. You can add them later in Settings \u{2192} Local Files.".to_string()
    } else if lib.scanning && lib.songs.is_empty() {
        format!("Looking\u{2026} {} files so far", super::local_files::thousands(lib.scanned))
    } else {
        let n = lib.songs.len();
        let more = if lib.scanning { ", still looking" } else { "" };
        format!("{} song{} found{more}", super::local_files::thousands(n), if n == 1 { "" } else { "s" })
    };
    let add = el().key("onb/folders/add").role("button").label("Add folder").focusable(true).h(44.0).px(22.0).radius(22.0).center()
        .fill(ov(0.08)).hover(ov(0.14)).press(ov(0.18)).cursor(Cursor::Pointer)
        .on_press_tx(|_s: &mut State, tx: &mut Tx| super::local_files::pick_folder(tx))
        .child(text("Add folder\u{2026}").font(BUTTON).color(PAL.text));
    col().items_center().children(staggered(vec![
        text("Your music folders").key("onb/folders/title").font(H1).color(PAL.text),
        text("Uzumaki plays the FLAC, MP3, M4A, Ogg and WAV files in them.").key("onb/folders/sub").font(BODY14).color(PAL.text2).mt(8.0),
        el().key("onb/folders/rows").mt(26.0).child(rows),
        text(count).key("onb/folders/count").font(SMALL13).color(PAL.dim).mt(14.0),
        row().key("onb/folders/buttons").gap(12.0).mt(26.0).child(add).child(primary("onb/folders/go", "Continue", next)),
    ]))
}

/// Sign in (Apple): explains, opens Apple's window, waits, and says how it went.
fn sign_in(s: &State, o: &Onboarding, win: [f32; 2]) -> El {
    let view = sign_in_view(s, o);
    let w = (win[0] - 64.0).clamp(280.0, 440.0);
    // Keyed per state: Rune fades one into the next.
    let (key, content) = match &view {
        SignInView::Starting | SignInView::Lead => ("onb/signin/lead", lead(&view, w)),
        SignInView::Waiting => ("onb/signin/waiting", waiting(w)),
        SignInView::Closed => ("onb/signin/closed", closed(w)),
        SignInView::Done(store) => ("onb/signin/done", signed_in(store.as_deref())),
        SignInView::Error(e) => ("onb/signin/error", failed(e, w)),
    };
    let fade = Transition::offset(0.0, 8.0).combined(Transition::opacity());
    col().key("onb/signin").items_center()
        .child_exit(Transition::opacity())
        .child(col().key(key).items_center().transition(fade).child(content))
}

fn sign_in_title(title: &'static str) -> El {
    let badge = el().size(52.0, 52.0).radius(16.0).center().fill(PAL.accent.alpha(0.16)).child(icon(&icons::SONGS, 24.0).color(PAL.accent));
    col().items_center().child(badge).child(text(title).font(H1).color(PAL.text).mt(18.0))
}

/// One "what happens next" line: a small coral mark and the words.
fn point(key: &'static str, ic: &'static rune::Icon, words: &'static str, w: f32) -> El {
    row().key(key).w(w).gap(12.0).items_start()
        .child(el().size(26.0, 26.0).radius(13.0).center().fill(ov(0.06)).child(icon(ic, 14.0).color(PAL.accent)))
        .child(text(words).font(BODY14).color(PAL.text2).wrap().grow(1.0).min_w(0.0).mt(3.0))
}

/// A button that isn't the main one: a filled grey pill, same height as the coral one.
fn secondary(key: &'static str, label: &'static str, press: fn(&mut State)) -> El {
    el().key(key).role("button").label(label).focusable(true).h(44.0).px(22.0).radius(22.0).center()
        .fill(ov(0.08)).hover(ov(0.14)).press(ov(0.18)).cursor(Cursor::Pointer)
        .on_press(press)
        .child(text(label).font(BUTTON).color(PAL.text))
}

fn lead(view: &SignInView, w: f32) -> El {
    let ready = *view != SignInView::Starting;
    let open = if ready {
        primary("onb/signin/open", "Open Apple sign-in", open_sign_in)
    } else {
        // Not a button yet: the engine is starting (first starts download Widevine).
        row().key("onb/signin/getting-ready").role("status").label("Getting ready").h(44.0).px(24.0).radius(22.0).gap(10.0).center()
            .fill(PAL.accent.alpha(0.35))
            .child(icon(&icons::SWIRL, 16.0).color(PAL.text).looping(LoopSpec::rotation(1.6)))
            .child(text("Getting ready\u{2026}").font(BUTTON).color(PAL.text))
    };
    let parts = vec![
        sign_in_title("Sign in to Apple Music").key("onb/signin/title"),
        col().key("onb/signin/points").gap(12.0).mt(24.0)
            .child(point("onb/signin/p1", &icons::USER, "Apple\u{2019}s own sign-in window opens. Uzumaki never sees your password.", w))
            .child(point("onb/signin/p2", &icons::CHECK, "Two-factor codes and checks happen there too.", w))
            .child(point("onb/signin/p3", &icons::NEXT, "When you\u{2019}re in, this window carries on by itself.", w)),
        row().key("onb/signin/buttons").gap(12.0).mt(30.0).center_y()
            .child(open)
            .child(quiet("onb/signin/not-now", "Not now", not_now)),
    ];
    col().items_center().children(staggered(parts))
}

/// Apple's window is open: a card in our style while the user signs in there.
fn waiting(w: f32) -> El {
    let dot = el().size(8.0, 8.0).radius(4.0).fill(PAL.accent).looping(LoopSpec::opacity(1.4, 0.0, 0.25, 1.0));
    let card = col().key("onb/signin/card").role("status").label("Waiting for Apple sign-in").w(w).padding(26.0).gap(0.0).items_center()
        .radius(18.0).fill(PAL.player).border(1.0, PAL.field_border).shadow(6.0, 24.0, shadow(0.25))
        .child(icon(&icons::SWIRL, 36.0).color(PAL.accent).looping(LoopSpec::rotation(2.4)))
        .child(text("Finish signing in in the Apple window").font(TextStyle::new(17.0, 600.0)).color(PAL.text).mt(18.0).wrap())
        .child(row().gap(8.0).center_y().mt(10.0).child(dot).child(text("Waiting for Apple\u{2026}").font(SMALL13).color(PAL.dim)))
        .child(
            row().gap(10.0).mt(24.0).center_y()
                .child(secondary("onb/signin/again", "Bring the window back", open_sign_in))
                .child(quiet("onb/signin/cancel", "Cancel", cancel_sign_in)),
        );
    col().items_center().child(card.transition(Transition::scale(0.96).combined(Transition::opacity())))
}

/// The user closed Apple's window without signing in.
fn closed(w: f32) -> El {
    let card = col().key("onb/signin/card").w(w).padding(26.0).items_center()
        .radius(18.0).fill(PAL.player).border(1.0, PAL.field_border).shadow(6.0, 24.0, shadow(0.25))
        .child(el().size(44.0, 44.0).radius(22.0).center().fill(ov(0.08)).child(icon(&icons::USER, 20.0).color(PAL.text2)))
        .child(text("The sign-in window was closed.").font(TextStyle::new(17.0, 600.0)).color(PAL.text).mt(16.0))
        .child(text("You\u{2019}re not signed in yet.").font(SMALL13).color(PAL.dim).mt(6.0))
        .child(
            row().gap(10.0).mt(24.0).center_y()
                .child(primary("onb/signin/retry", "Try again", open_sign_in))
                .child(quiet("onb/signin/not-now", "Not now", not_now)),
        );
    col().items_center().child(card)
}

/// Signed in: a tick, and the store once known. The tour goes on by itself.
fn signed_in(store: Option<&str>) -> El {
    let tick = el().key("onb/signin/tick").size(56.0, 56.0).radius(28.0).center().fill(PAL.accent)
        .child(icon(&icons::CHECK, 26.0).color(PAL.ink))
        .transition(Transition::scale(0.6).combined(Transition::opacity()));
    let mut c = col().items_center()
        .child(tick)
        .child(text("Signed in").key("onb/signin/signed").font(HERO).color(PAL.text).mt(22.0)
            .transition(Transition::offset(0.0, 10.0).combined(Transition::opacity())).enter_delay(0.12));
    if let Some(store) = store {
        c = c.child(
            row().key("onb/signin/store").role("status").label(format!("Store: {store}")).mt(14.0).h(30.0).px(14.0).radius(15.0).gap(8.0).center()
                .fill(ov(0.08))
                .child(text("Store").font(SMALL13).color(PAL.nav_text))
                .child(text(store.to_string()).font(TextStyle::new(13.0, 600.0)).color(PAL.text))
                .transition(Transition::opacity()).enter_delay(0.3),
        );
    }
    c.child(text("Your library is syncing.").key("onb/signin/line").font(BODY14).color(PAL.text2).mt(14.0)
        .transition(Transition::opacity()).enter_delay(0.4))
}

/// The engine failed (Widevine, Electron gone, couldn't start).
fn failed(e: &str, w: f32) -> El {
    let card = col().key("onb/signin/card").role("alert").label("Apple Music couldn't start").w(w).padding(26.0).items_center()
        .radius(18.0).fill(PAL.player).border(1.0, PAL.field_border).shadow(6.0, 24.0, shadow(0.25))
        .child(text("Apple Music couldn\u{2019}t start").font(TextStyle::new(17.0, 600.0)).color(PAL.text))
        .child(text(e.to_string()).font(SMALL13).color(PAL.danger_text).wrap().mt(10.0).w_full())
        .child(
            row().gap(10.0).mt(24.0).center_y()
                .child(primary("onb/signin/restart", "Try again", retry_bridge))
                .child(secondary("onb/signin/local", "Use local files instead", use_local)),
        );
    col().items_center().child(card)
}

/// The library downloading behind the tour: a line and a thin bar above the bottom row.
fn sync_line(s: &State, win: [f32; 2]) -> Option<El> {
    if !s.syncing || !s.session.authorized {
        return None;
    }
    let w = (win[0] - 64.0).clamp(240.0, 420.0);
    let words = format!("Syncing your library \u{b7} {}", intro::counts(s));
    // Until every list's length is known: the songs so far.
    let f = s.sync_fraction().unwrap_or_else(|| match s.sync_counts()[0] {
        (have, Some(total)) if total > 0 => have as f32 / total as f32,
        _ => 0.0,
    });
    Some(
        col().key("onb/sync").role("status").label("Syncing your library").pin(Some(0.0), None, Some(0.0), Some(76.0)).items_center().gap(8.0)
            .transition(Transition::opacity())
            .child(text(words).font(SMALL12).color(PAL.nav_text).truncate().max_w(w))
            .child(intro::bar("onb/sync/bar", w, f, ov(0.1))),
    )
}

/// One folder: a tick (in the list or not), the path with `~`, and the whole row as the button.
fn folder_row(key: String, dir: &std::path::Path, on: bool) -> El {
    let path = super::settings::tilde(dir);
    let tick = el().size(22.0, 22.0).radius(11.0).center();
    let tick = if on {
        tick.fill(PAL.accent).child(icon(&icons::CHECK, 13.0).color(PAL.ink))
    } else {
        tick.border(1.5, PAL.field_border)
    };
    row().key(key).role("checkbox").label(path.clone()).focusable(true).w_full().h(48.0).px(14.0).gap(12.0).center_y()
        .radius(12.0).fill(PAL.player).border(1.0, PAL.field_border).hover(ov(0.04)).press(ov(0.08)).cursor(Cursor::Pointer)
        .child(tick)
        .child(icon(&icons::LOCAL_FILES, 17.0).color(PAL.accent))
        .child(text(path).font(BODY14).color(if on { PAL.text } else { PAL.text2 }).truncate().grow(1.0).min_w(0.0))
}

fn done(s: &State, o: &Onboarding) -> El {
    let line = match (o.mode, o.chosen) {
        (Mode::Replay, _) => "That\u{2019}s the tour. Enjoy the music.".to_string(),
        (_, Some(MusicSource::LocalOnly)) if s.settings.music_folders.is_empty() => {
            "Add music folders any time in Settings \u{2192} Local Files.".to_string()
        }
        (_, Some(MusicSource::LocalOnly)) => "Your music is in Local Files.".to_string(),
        _ if s.session.authorized => "Your library is on its way.".to_string(),
        _ => "Sign in any time from the banner or Settings \u{2192} Account.".to_string(),
    };
    let tick = el().key("onb/done/tick").size(56.0, 56.0).radius(28.0).center().fill(PAL.accent)
        .child(icon(&icons::CHECK, 26.0).color(PAL.ink));
    col().items_center().children(staggered(vec![
        tick,
        text("You\u{2019}re all set.").key("onb/done/title").font(HERO).color(PAL.text).mt(22.0),
        text(line).key("onb/done/line").font(BODY14).color(PAL.text2).mt(10.0),
    ]))
}

/// Back · step dots · Skip, along the bottom. Gone on Done.
fn bottom(s: &State, o: &Onboarding) -> El {
    let steps = o.steps();
    let i = o.index();
    // On its own (from Settings) the step has no dots or Skip: its Not now and Esc leave.
    let shown = o.phase == Phase::In && o.step != Step::Done && steps.len() > 1;
    let dots = row().key("onb/dots").gap(6.0).center().role("status").label(format!("Step {} of {}", i + 1, steps.len()))
        .children(steps.iter().enumerate().map(|(n, _)| {
            let on = n == i;
            el().key(DOT_KEYS[n.min(DOT_KEYS.len() - 1)]).h(6.0).w(if on { 20.0 } else { 6.0 }).radius(3.0)
                .fill(if on { PAL.accent } else { ov(0.22) })
        }));
    // Signed in, the step moves on by itself: no going back to the choice from there.
    let signed = o.step == Step::SignIn && s.session.authorized;
    let back_btn = (o.step == Step::Tour && o.tour.card > 0 || can_go_back(o, s.session.authorized) && !signed).then(|| quiet("onb/back", "Back", back));
    let left = row().w(110.0).children(back_btn);
    let right = row().w(110.0).justify_end().child(quiet("onb/skip", "Skip", skip));
    row().key("onb/bottom").pin(Some(32.0), None, Some(32.0), Some(28.0)).center_y()
        .opacity(if shown { 1.0 } else { 0.0 })
        .opacity_motion(tween(0.25, Easing::EASE_OUT))
        .child(left)
        .child(el().grow(1.0))
        .child(dots)
        .child(el().grow(1.0))
        .child(right)
}

const DOT_KEYS: [&str; 6] = ["onb/dot/0", "onb/dot/1", "onb/dot/2", "onb/dot/3", "onb/dot/4", "onb/dot/5"];

#[cfg(test)]
mod tests {
    use super::*;

    fn poll_at(s: &mut State, t: f64) -> bool {
        let mut tx = Tx::default();
        poll(s, t, &mut tx)
    }

    fn step(s: &State) -> Option<Step> {
        s.ui.onboarding.as_ref().map(|o| o.step)
    }

    fn phase(s: &State) -> Option<Phase> {
        s.ui.onboarding.as_ref().map(|o| o.phase)
    }

    /// Next through every tour card to Done.
    fn through_tour(s: &mut State) {
        assert_eq!(step(s), Some(Step::Tour));
        for _ in 0..tour::cards(s).len() {
            next(s);
        }
        assert_eq!(step(s), Some(Step::Done));
    }

    /// Up and waiting on Welcome.
    fn started(mode: Mode) -> State {
        let mut s = State::offline();
        match mode {
            Mode::FirstRun => start(&mut s),
            Mode::Replay => replay(&mut s),
            Mode::SignIn => unreachable!("see switching_to_apple_opens_the_step_on_its_own"),
        }
        poll_at(&mut s, 0.0);
        poll_at(&mut s, 0.5);
        assert_eq!(phase(&s), Some(Phase::In));
        s
    }

    #[test]
    fn who_sees_it() {
        // A new install: nothing finished, nothing on disk.
        assert!(at_launch(None, true, false, false));
        // Finished or skipped (any version), or an old settings file (Some(0)).
        assert!(!at_launch(Some(1), true, false, false));
        assert!(!at_launch(Some(0), true, false, false));
        // Settings lost, but a sign-in or library on disk: an existing install.
        assert!(!at_launch(None, false, false, false));
        // A second copy for tests stays out of the way...
        assert!(!at_launch(None, true, true, false));
        // ...unless asked for (UZUMAKI_ONBOARDING=1).
        assert!(at_launch(None, true, true, true));
        assert!(at_launch(Some(1), false, false, true));
    }

    #[test]
    fn first_run_moves_as_drawn() {
        let mut s = started(Mode::FirstRun);
        assert_eq!(step(&s), Some(Step::Welcome));
        back(&mut s);
        assert_eq!(step(&s), Some(Step::Welcome), "nothing before Welcome");
        next(&mut s);
        assert_eq!(step(&s), Some(Step::Choice));
        back(&mut s);
        assert_eq!(step(&s), Some(Step::Welcome));
        assert!(s.ui.onboarding.as_ref().unwrap().back);
        next(&mut s);
        assert!(!s.ui.onboarding.as_ref().unwrap().back);
        assert_eq!(s.settings.onboarded, None, "not done until Done or Skip");
        choose(&mut s, MusicSource::LocalOnly);
        assert_eq!(step(&s), Some(Step::Folders), "local files only: the folders next");
        assert_eq!(s.settings.music_source, MusicSource::LocalOnly);
        next(&mut s);
        assert_eq!(step(&s), Some(Step::Tour), "then the shortcuts");
        assert_eq!(s.settings.onboarded, None);
        back(&mut s);
        assert_eq!(step(&s), Some(Step::Folders), "Back from the first card");
        next(&mut s);
        through_tour(&mut s);
        assert_eq!(s.settings.onboarded, Some(TOUR_VERSION));
        back(&mut s);
        assert_eq!(step(&s), Some(Step::Done), "no Back from Done");
        // Done pauses, then twists out onto Local Files, then goes.
        poll_at(&mut s, 1.0);
        assert!(!poll_at(&mut s, 1.5));
        assert!(poll_at(&mut s, 2.0 + DONE_HOLD));
        assert_eq!(phase(&s), Some(Phase::Out));
        assert_eq!(s.route, Route::LocalFiles);
        poll_at(&mut s, 3.1);
        assert!(poll_at(&mut s, 3.2 + intro::OUT));
        assert!(s.ui.onboarding.is_none());
    }

    #[test]
    fn choosing_apple_keeps_the_page_and_waits_to_sign_in() {
        let mut s = started(Mode::FirstRun);
        s.settings.music_source = MusicSource::LocalOnly; // as if chosen, then changed
        next(&mut s);
        choose(&mut s, MusicSource::Apple);
        assert_eq!(s.settings.music_source, MusicSource::Apple);
        assert_eq!(step(&s), Some(Step::SignIn), "Apple Music: the sign-in step next");
        not_now(&mut s);
        assert_eq!(step(&s), Some(Step::Tour), "Not now goes on without signing in");
        through_tour(&mut s);
        poll_at(&mut s, 1.0);
        poll_at(&mut s, 2.5);
        assert_eq!(phase(&s), Some(Phase::Out));
        assert_eq!(s.route, Route::Home);
        // No bridge here (tests), so no sign-in window is asked for.
        assert!(!s.session.login_window && !s.session.login_after_ready);
    }

    #[test]
    fn skip_and_escape_leave_with_defaults() {
        let mut s = started(Mode::FirstRun);
        next(&mut s);
        skip(&mut s);
        assert_eq!(phase(&s), Some(Phase::Out));
        assert_eq!(s.settings.onboarded, Some(TOUR_VERSION));
        assert_eq!(s.settings.music_source, MusicSource::Apple, "Apple mode");
        assert_eq!(s.route, Route::Home);
        // Nothing moves once it is leaving.
        next(&mut s);
        choose(&mut s, MusicSource::LocalOnly);
        assert_eq!(s.settings.music_source, MusicSource::Apple);
        assert_eq!(step(&s), Some(Step::Choice));
    }

    #[test]
    fn show_again_runs_welcome_the_tour_and_done_only() {
        let mut s = State::offline();
        s.settings.onboarded = Some(TOUR_VERSION);
        s.settings.music_source = MusicSource::LocalOnly;
        super::super::settings::open(&mut s);
        replay(&mut s);
        assert!(s.ui.settings.open.is_none(), "Settings closes");
        // It folds the app away first.
        poll_at(&mut s, 0.0);
        assert!(!poll_at(&mut s, 0.1));
        assert!(poll_at(&mut s, 0.1 + intro::FOLD_HOLD));
        assert_eq!(s.ui.onboarding.as_ref().unwrap().steps(), [Step::Welcome, Step::Tour, Step::Done]);
        next(&mut s);
        assert_eq!(step(&s), Some(Step::Tour), "no choice on a replay: the shortcuts");
        // Local only mode: the tour's local cards (Up Next, no lyrics).
        assert_eq!(tour::cards(&s).len(), 8);
        assert!(!tour::Card::Panels.takes(crate::ui::keys::Action::ShowLyrics, s.local_only()));
        through_tour(&mut s);
        assert_eq!(s.settings.music_source, MusicSource::LocalOnly, "the choice is left alone");
        assert_eq!(s.settings.onboarded, Some(TOUR_VERSION));
    }

    /// The running first-run scene in a headless window with the app's poll and keys.
    fn window(reduced: bool) -> rune::Runtime<State> {
        let mut s = State::offline();
        (crate::scenes::SCENES.iter().find(|(n, _)| *n == "onboarding").unwrap().1)(&mut s);
        let mut rt = rune::Runtime::new(s, crate::ui::view, (1360.0, 860.0), rune::Env { system_dark: true, scale: 1.0 });
        rt.set_system_reduced_motion(reduced);
        rt.on_key = Some(crate::ui::on_key);
        rt.poll = Some(crate::ui::poll);
        run(&mut rt, 1.5);
        rt
    }

    fn run(rt: &mut rune::Runtime<State>, secs: f32) {
        rt.frame(0.0);
        for _ in 0..(secs * 60.0) as usize {
            rt.frame(1.0 / 60.0);
        }
    }

    fn click(rt: &mut rune::Runtime<State>, key: &str) {
        let r = rt.presented(key).unwrap_or_else(|| panic!("{key} on screen")).0;
        let (x, y) = (r[0] + r[2] / 2.0, r[1] + r[3] / 2.0);
        rt.pointer_move(x, y);
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        run(rt, 0.6);
    }

    fn key(rt: &mut rune::Runtime<State>, key: rune::Key) {
        rt.key(rune::KeyEvent { key, mods: Default::default() });
        run(rt, 0.6);
    }

    fn now(rt: &mut rune::Runtime<State>) -> (Option<Step>, Option<Phase>, Route) {
        let mut out = None;
        rt.update(|s, _| out = Some((step(s), phase(s), s.route.clone())));
        out.unwrap()
    }

    /// Enter presses Get started inside the layer (a Rune modal, which the app's root key
    /// handler never sees), the cards and Back take clicks, and Done twists out onto Local
    /// Files. Under Reduce Motion too.
    #[test]
    fn a_first_run_by_keys_and_clicks() {
        for reduced in [false, true] {
            let mut rt = window(reduced);
            assert_eq!(now(&mut rt).0, Some(Step::Welcome));
            assert!(rt.presented("onb/start").is_some(), "Get started shows (reduced: {reduced})");
            key(&mut rt, rune::Key::Enter);
            assert_eq!(now(&mut rt).0, Some(Step::Choice), "Enter pressed Get started (reduced: {reduced})");
            click(&mut rt, "onb/back");
            assert_eq!(now(&mut rt).0, Some(Step::Welcome));
            click(&mut rt, "onb/start");
            click(&mut rt, "onb/choice/local");
            assert_eq!(now(&mut rt).0, Some(Step::Folders));
            key(&mut rt, rune::Key::Enter);
            assert_eq!(now(&mut rt).0, Some(Step::Tour), "Enter pressed Continue (reduced: {reduced})");
            // Return presses Next on every card.
            for _ in 0..8 {
                key(&mut rt, rune::Key::Enter);
            }
            assert_eq!(now(&mut rt).0, Some(Step::Done), "Enter went through the tour (reduced: {reduced})");
            run(&mut rt, 3.0);
            let (step, _, route) = now(&mut rt);
            assert_eq!(step, None, "gone after the twist-out (reduced: {reduced})");
            assert_eq!(route, Route::LocalFiles);
        }
    }

    #[test]
    fn escape_skips_from_any_step() {
        let mut rt = window(false);
        click(&mut rt, "onb/start");
        key(&mut rt, rune::Key::Escape);
        run(&mut rt, 1.5);
        let (step, _, route) = now(&mut rt);
        assert_eq!(step, None);
        assert_eq!(route, Route::Home);
        rt.update(|s, _| {
            assert_eq!(s.settings.onboarded, Some(TOUR_VERSION));
            assert_eq!(s.settings.music_source, MusicSource::Apple);
        });
    }

    /// Decision 1: no bridge until Apple Music is chosen; Back and local files stop it again;
    /// Skip (Apple mode) starts it; local only never does.
    #[test]
    fn the_bridge_starts_with_the_apple_choice() {
        let (engine, starts) = crate::bridge::engine_tests::fake::<crate::state::Msg>(|_| {});
        let n = || starts.load(std::sync::atomic::Ordering::SeqCst);
        let mut s = started(Mode::FirstRun);
        s.engine = Some(engine.clone());
        next(&mut s);
        assert_eq!(n(), 0, "nothing runs while the onboarding is up");
        choose(&mut s, MusicSource::Apple);
        assert_eq!(n(), 1);
        assert!(s.bridge.is_some() && !s.session.login_after_ready, "the sign-in waits for the twist-out");
        // Done → no Back; replay the choice on a fresh run instead.
        let mut s = started(Mode::FirstRun);
        s.engine = Some(engine.clone());
        next(&mut s);
        choose(&mut s, MusicSource::LocalOnly);
        assert_eq!(n(), 1, "local only starts none");
        back(&mut s);
        choose(&mut s, MusicSource::Apple);
        assert_eq!(n(), 2);
        assert!(s.bridge.is_some());
        let mut s2 = started(Mode::FirstRun);
        s2.engine = Some(engine.clone());
        skip(&mut s2);
        assert_eq!(n(), 3, "Skip leaves in Apple mode, which needs the bridge");
        let mut s3 = started(Mode::FirstRun);
        s3.engine = Some(engine.clone());
        next(&mut s3);
        choose(&mut s3, MusicSource::LocalOnly);
        back(&mut s3);
        skip(&mut s3);
        assert_eq!(n(), 3, "local only chosen, then Skip: still none");
        engine.shutdown();
    }

    /// Folders: the usual music folder goes in pre-ticked (and its scan starts); unticking takes
    /// it out; Back to the choice takes out what the step added by itself, not the user's own.
    #[test]
    fn the_folders_step_offers_the_music_folder() {
        let music = PathBuf::from("/home/u/Music");
        let mut s = started(Mode::FirstRun);
        s.ui.onboarding.as_mut().unwrap().suggest = Some(music.clone());
        next(&mut s);
        choose(&mut s, MusicSource::LocalOnly);
        assert_eq!(s.settings.music_folders, [music.clone()], "pre-ticked");
        toggle_suggested(&mut s);
        assert!(s.settings.music_folders.is_empty());
        toggle_suggested(&mut s);
        assert_eq!(s.settings.music_folders, [music.clone()]);
        back(&mut s);
        assert_eq!(s.settings.music_folders, [music.clone()], "ticked by the user: kept");
        // Added by the step itself, then Back: gone again.
        s.settings.music_folders.clear();
        choose(&mut s, MusicSource::LocalOnly);
        assert_eq!(s.settings.music_folders, [music.clone()]);
        back(&mut s);
        assert!(s.settings.music_folders.is_empty());
        // Folders already there: nothing is added.
        s.settings.music_folders = vec![PathBuf::from("/mnt/flac")];
        choose(&mut s, MusicSource::LocalOnly);
        assert_eq!(s.settings.music_folders, [PathBuf::from("/mnt/flac")]);
        // Continue works with no folders too.
        s.settings.music_folders.clear();
        next(&mut s);
        assert_eq!(step(&s), Some(Step::Tour));
    }

    #[test]
    fn xdg_music_dir_is_read_from_user_dirs() {
        let home = std::path::Path::new("/home/u");
        let file = "# comment\nXDG_DESKTOP_DIR=\"$HOME/Desktop\"\nXDG_MUSIC_DIR=\"$HOME/Musik\"\n";
        assert_eq!(xdg_music_dir(file, home), Some(PathBuf::from("/home/u/Musik")));
        assert_eq!(xdg_music_dir("XDG_MUSIC_DIR=\"/data/music\"", home), Some(PathBuf::from("/data/music")));
        assert_eq!(xdg_music_dir("XDG_MUSIC_DIR=\"$HOME\"", home), Some(PathBuf::from("/home/u")));
        assert_eq!(xdg_music_dir("XDG_VIDEOS_DIR=\"$HOME/Videos\"", home), None);
    }

    /// Apple was chosen before the bridge was up: the sign-in window opens with `ready`, once,
    /// and not for someone already signed in.
    #[test]
    fn sign_in_waits_for_the_bridge() {
        use crate::bridge::Event;
        use crate::state::Msg;
        let ready = |authorized| Msg::Bridge(Event::Ready { authorized, storefront: None, volume: None });
        let mut s = State::offline();
        s.session.login_after_ready = true;
        s.apply(ready(false), &mut Tx::default());
        assert!(s.session.login_window && !s.session.login_after_ready);
        s.show_login(false);
        s.apply(ready(false), &mut Tx::default());
        assert!(!s.session.login_window, "only once");
        let mut s = State::offline();
        s.session.login_after_ready = true;
        s.apply(ready(true), &mut Tx::default());
        assert!(!s.session.login_window && !s.session.login_after_ready);
    }

    // ---------------------------------------------------------------- sign in

    use crate::bridge::Event;
    use crate::state::Msg;

    fn bridge(s: &mut State, e: Event) {
        s.apply(Msg::Bridge(e), &mut Tx::default());
    }

    fn view(s: &State) -> SignInView {
        sign_in_view(s, s.ui.onboarding.as_ref().unwrap())
    }

    fn sign_ins(s: &State) -> usize {
        s.sent.borrow().iter().filter(|l| l.starts_with("signIn")).count()
    }

    /// Welcome → Choice (Apple) → Sign in, with the bridge not up yet.
    fn at_sign_in() -> State {
        let mut s = started(Mode::FirstRun);
        next(&mut s);
        choose(&mut s, MusicSource::Apple);
        assert_eq!(step(&s), Some(Step::SignIn));
        s
    }

    /// The states from fake bridge events: getting ready until `ready`; Open shows Apple's window
    /// (`signIn`); closing it says so (only a close by the user, not our hide); Try again and
    /// Cancel; signing in shows the store and moves on after a moment.
    #[test]
    fn sign_in_states_follow_the_bridge() {
        let mut s = at_sign_in();
        assert_eq!(view(&s), SignInView::Starting);
        open_sign_in(&mut s);
        assert_eq!((view(&s), sign_ins(&s)), (SignInView::Starting, 0), "Open waits for the engine");
        bridge(&mut s, Event::Ready { authorized: false, storefront: Some("us".into()), volume: None });
        assert_eq!(view(&s), SignInView::Lead);
        assert!(!s.session.login_window, "the window waits for the button");
        assert_eq!(s.settings.storefront, None, "a signed-out storefront isn't the account's");

        open_sign_in(&mut s);
        assert_eq!((view(&s), sign_ins(&s)), (SignInView::Waiting, 1));
        assert!(s.session.login_window);
        bridge(&mut s, Event::Window { visible: true, closed: false });
        assert_eq!(view(&s), SignInView::Waiting);
        // Bring the window back: signIn again, still waiting.
        open_sign_in(&mut s);
        assert_eq!((view(&s), sign_ins(&s)), (SignInView::Waiting, 2));
        // The user closes Apple's window.
        bridge(&mut s, Event::Window { visible: false, closed: true });
        assert_eq!(view(&s), SignInView::Closed);
        // Try again.
        open_sign_in(&mut s);
        assert_eq!((view(&s), sign_ins(&s)), (SignInView::Waiting, 3));
        // Cancel: we hide it (a `hidden` window event follows), back to the lead-in.
        cancel_sign_in(&mut s);
        bridge(&mut s, Event::Window { visible: false, closed: false });
        assert_eq!(view(&s), SignInView::Lead);
        assert!(!s.session.login_window);
        // A close while not waiting changes nothing.
        window_closed(&mut s);
        assert_eq!(view(&s), SignInView::Lead);

        open_sign_in(&mut s);
        bridge(&mut s, Event::Auth { authorized: true, storefront: Some("us".into()) });
        assert_eq!(view(&s), SignInView::Done(None), "the page's storefront isn't the account's");
        assert!(!s.session.login_window, "signed in: Apple's window hides");
        s.apply(Msg::AccountStorefront(Ok(("it".into(), Some("Italy".into())))), &mut Tx::default());
        assert_eq!(s.settings.storefront.as_ref().map(|f| f.id.as_str()), Some("it"));
        assert_eq!(view(&s), SignInView::Done(Some("Italy".into())));
        // The library syncs behind the tour: no sync screen.
        assert!(s.syncing && s.ui.intro.is_none());
        // "Signed in" stays a moment, then the tour goes on by itself; no Back meanwhile.
        back(&mut s);
        assert_eq!(step(&s), Some(Step::SignIn));
        poll_at(&mut s, 10.0);
        assert_eq!(step(&s), Some(Step::SignIn));
        poll_at(&mut s, 10.0 + SIGNED_HOLD - 0.1);
        assert_eq!(step(&s), Some(Step::SignIn));
        assert!(poll_at(&mut s, 10.01 + SIGNED_HOLD));
        assert_eq!(step(&s), Some(Step::Tour));
        // No Back from the first card into a sign-in that already happened.
        back(&mut s);
        assert_eq!(step(&s), Some(Step::Tour));
    }

    /// Already signed in on the way in (Back and forth after signing in): Done at once.
    #[test]
    fn signed_in_already_shows_done() {
        let mut s = at_sign_in();
        s.session.bridge_up = true;
        s.session.authorized = true;
        assert_eq!(view(&s), SignInView::Done(None));
    }

    /// Leaving the step while Apple's window shows hides it: Back, Not now, Skip.
    #[test]
    fn leaving_the_step_hides_apple_s_window() {
        for leave in [back as fn(&mut State), not_now, skip] {
            let mut s = at_sign_in();
            s.session.bridge_up = true;
            open_sign_in(&mut s);
            assert!(s.session.login_window);
            leave(&mut s);
            assert!(!s.session.login_window);
        }
    }

    /// The engine failed: Try again starts a new bridge; Use local files goes to the Folders step.
    #[test]
    fn a_bridge_error_offers_retry_and_local_files() {
        let (engine, starts) = crate::bridge::engine_tests::fake::<Msg>(|_| {});
        let n = || starts.load(std::sync::atomic::Ordering::SeqCst);
        let mut s = started(Mode::FirstRun);
        s.engine = Some(engine.clone());
        next(&mut s);
        choose(&mut s, MusicSource::Apple);
        assert_eq!(n(), 1);
        bridge(&mut s, Event::Widevine { ok: false, error: Some("offline".into()) });
        assert!(matches!(view(&s), SignInView::Error(ref e) if e.contains("offline")));
        retry_bridge(&mut s);
        assert_eq!(n(), 2, "a new bridge");
        assert_eq!(view(&s), SignInView::Starting);
        bridge(&mut s, Event::Exited);
        assert!(matches!(view(&s), SignInView::Error(_)));
        use_local(&mut s);
        assert_eq!(step(&s), Some(Step::Folders));
        assert_eq!(s.settings.music_source, MusicSource::LocalOnly);
        assert!(s.bridge.is_none(), "local only: the bridge stops");
        engine.shutdown();
    }

    /// Settings → Account → Apple Music, not signed in: the Sign in step on its own (Settings
    /// closes). Not now, or signing in, goes back to the app; nothing else changes.
    #[test]
    fn switching_to_apple_opens_the_step_on_its_own() {
        let (engine, _) = crate::bridge::engine_tests::fake::<Msg>(|_| {});
        let mut s = State::offline();
        s.engine = Some(engine.clone());
        s.settings.onboarded = Some(TOUR_VERSION);
        s.set_music_source(MusicSource::LocalOnly);
        super::super::settings::open(&mut s);
        s.switch_to_apple();
        assert!(s.ui.settings.open.is_none(), "Settings closes");
        assert_eq!(s.ui.onboarding.as_ref().map(|o| (o.mode, o.step)), Some((Mode::SignIn, Step::SignIn)));
        assert!(s.bridge.is_some());
        poll_at(&mut s, 0.0);
        poll_at(&mut s, 0.1 + intro::FOLD_HOLD);
        assert_eq!(phase(&s), Some(Phase::In));
        bridge(&mut s, Event::Ready { authorized: false, storefront: None, volume: None });
        assert_eq!(view(&s), SignInView::Lead);
        not_now(&mut s);
        assert_eq!(phase(&s), Some(Phase::Out));
        assert_eq!(s.settings.onboarded, Some(TOUR_VERSION));
        assert_eq!(s.settings.music_source, MusicSource::Apple);

        // Signed in from the step: back to the app after the moment.
        s.ui.onboarding = None;
        s.set_music_source(MusicSource::LocalOnly);
        s.switch_to_apple();
        s.ui.onboarding.as_mut().unwrap().phase = Phase::In;
        bridge(&mut s, Event::Auth { authorized: true, storefront: Some("it".into()) });
        poll_at(&mut s, 20.0);
        assert!(poll_at(&mut s, 20.01 + SIGNED_HOLD));
        assert_eq!(phase(&s), Some(Phase::Out));

        // Signed in already: no step.
        s.ui.onboarding = None;
        s.set_music_source(MusicSource::LocalOnly);
        s.switch_to_apple();
        assert!(s.ui.onboarding.is_none());
        engine.shutdown();
    }

    /// The step in a headless window: Getting ready until `ready`, then the buttons by click,
    /// Apple's window closed, Try again, and Not now on to Done.
    #[test]
    fn the_sign_in_step_by_clicks() {
        let mut rt = window(false);
        // The scene's app behind is signed in; a first run isn't.
        rt.update(|s, _| {
            s.session.authorized = false;
            s.session.bridge_up = false;
        });
        click(&mut rt, "onb/start");
        click(&mut rt, "onb/choice/apple");
        assert_eq!(now(&mut rt).0, Some(Step::SignIn));
        assert!(rt.presented("onb/signin/getting-ready").is_some());
        assert!(rt.presented("onb/signin/open").is_none());
        rt.update(|s, tx| {
            s.apply(Msg::Bridge(Event::Ready { authorized: false, storefront: None, volume: None }), tx);
        });
        run(&mut rt, 0.6);
        click(&mut rt, "onb/signin/open");
        assert!(rt.presented("onb/signin/again").is_some(), "waiting");
        rt.update(|s, tx| {
            s.apply(Msg::Bridge(Event::Window { visible: false, closed: true }), tx);
        });
        run(&mut rt, 0.6);
        click(&mut rt, "onb/signin/retry");
        assert!(rt.presented("onb/signin/cancel").is_some());
        click(&mut rt, "onb/signin/cancel");
        click(&mut rt, "onb/signin/not-now");
        assert_eq!(now(&mut rt).0, Some(Step::Tour));
        let mut opened = 0;
        rt.update(|s, _| opened = sign_ins(s));
        assert_eq!(opened, 2);
    }

    // ---------------------------------------------------------------- the tour

    use crate::ui::keys::Action;
    use tour::Card;

    fn tour_of(s: &State) -> Tour {
        s.ui.onboarding.as_ref().unwrap().tour.clone()
    }

    /// On the tour's first card, live (not a frozen scene).
    fn at_tour(local: bool) -> State {
        let mut s = started(Mode::FirstRun);
        next(&mut s);
        if local {
            choose(&mut s, MusicSource::LocalOnly);
        } else {
            choose(&mut s, MusicSource::Apple);
        }
        next(&mut s);
        assert_eq!(step(&s), Some(Step::Tour));
        s
    }

    /// The cards come from `BINDINGS` as the user has them: all eight by default; a removed
    /// shortcut drops its card; Lyrics and Up Next keep their card while one of them is bound.
    /// Local only: the same cards, but no lyrics.
    #[test]
    fn the_cards_follow_the_user_s_shortcuts() {
        let all = [Card::PlayPause, Card::Sidebar, Card::RightPanel, Card::Panels, Card::NowPlaying, Card::Find, Card::Skip, Card::More];
        let mut s = State::offline();
        assert_eq!(tour::cards(&s), all);
        s.set_keybind("play_pause", "", "space");
        assert_eq!(tour::cards(&s)[0], Card::Sidebar, "no play/pause shortcut: no card");
        s.set_keybind("show_lyrics", "", "mod+l");
        assert!(tour::cards(&s).contains(&Card::Panels), "Up Next is still bound");
        s.set_keybind("show_queue", "", "mod+u");
        assert!(!tour::cards(&s).contains(&Card::Panels));
        s.set_keybind("next", "", "mod+right");
        assert!(tour::cards(&s).contains(&Card::Skip), "Previous is still bound");
        s.set_keybind("settings", "", "mod+comma");
        s.set_keybind("shortcuts", "", "mod+slash");
        assert_eq!(tour::cards(&s).last(), Some(&Card::More), "the closing card always shows");

        let mut s = State::offline();
        s.settings.music_source = MusicSource::LocalOnly;
        assert_eq!(tour::cards(&s), all);
        assert_eq!(Card::Panels.actions(true), [Action::ShowQueue]);
        s.set_keybind("show_queue", "", "mod+u");
        assert!(!tour::cards(&s).contains(&Card::Panels), "local only: Lyrics doesn't keep it");
    }

    /// A card's own shortcut tries it: the picture changes, the tick shows, the keycap flashes
    /// and goes back; the card stays (no auto-advance) and the key toggles as often as pressed.
    /// Only Next moves on; the closing card waits for Finish.
    #[test]
    fn trying_a_card_stays_and_toggles() {
        let mut s = at_tour(false);
        assert_eq!(tour_of(&s).card, 0);
        assert!(!tour_of(&s).mini.playing, "Play and pause starts paused");
        let press = |s: &mut State, a| s.ui.onboarding.as_mut().unwrap().tour.press(a, false);
        press(&mut s, Action::PlayPause);
        assert!(tour_of(&s).mini.playing);
        assert!(tour_of(&s).lit);
        // The flash ends by itself; the card stays, however long.
        assert!(!poll_at(&mut s, 5.0));
        assert!(!poll_at(&mut s, 5.0 + tour::FLASH - 0.05));
        assert!(tour_of(&s).lit);
        assert!(poll_at(&mut s, 5.01 + tour::FLASH), "the cap going dark is a visible change");
        assert!(!tour_of(&s).lit);
        assert!(!poll_at(&mut s, 30.0), "idle after that");
        let t = tour_of(&s);
        assert_eq!((t.card, t.hit, t.mini.playing), (0, Some(Action::PlayPause), true), "stayed, tick kept");
        // Again: it toggles back, and the cap flashes again.
        press(&mut s, Action::PlayPause);
        assert!(!tour_of(&s).mini.playing && tour_of(&s).lit);
        // Only Next moves on, to an untried card.
        next(&mut s);
        let t = tour_of(&s);
        assert_eq!((t.card, t.hit, t.lit), (1, None, false));
        // The closing card: a number key lights its row and the sidebar entry, and stays.
        let last = tour::cards(&s).len() - 1;
        s.ui.onboarding.as_mut().unwrap().tour = Tour::at(last, Card::More);
        press(&mut s, Action::Slot(3));
        assert_eq!(tour_of(&s).mini.slot, Some(3));
        poll_at(&mut s, 40.0);
        poll_at(&mut s, 60.0);
        press(&mut s, Action::Slot(3));
        assert_eq!((step(&s), tour_of(&s).card), (Some(Step::Tour), last));
        next(&mut s);
        assert_eq!(step(&s), Some(Step::Done));
    }

    /// Each key does in the picture what it does in the app: toggles open and shut, the tab
    /// keys switch tabs and close on a second press of the same one, skips count.
    #[test]
    fn the_picture_toggles_like_the_app() {
        use tour::Pane;
        let mut m = tour::Mini::default();
        let both = |m: &mut tour::Mini, a: Action| m.apply(a, false);
        both(&mut m, Action::ToggleSidebar);
        assert!(m.folded);
        both(&mut m, Action::ToggleSidebar);
        assert!(!m.folded);
        // The right sidebar: a plain toggle, and it comes back on the tab it had.
        both(&mut m, Action::ToggleRightPanel);
        assert_eq!(m.right, Some(Pane::Lyrics));
        both(&mut m, Action::ToggleRightPanel);
        assert_eq!(m.right, None);
        // Lyrics / Up Next: switch tabs; the same key again closes.
        both(&mut m, Action::ShowQueue);
        assert_eq!(m.right, Some(Pane::Queue));
        both(&mut m, Action::ShowLyrics);
        assert_eq!(m.right, Some(Pane::Lyrics), "the other key switches tabs");
        both(&mut m, Action::ShowQueue);
        both(&mut m, Action::ShowQueue);
        assert_eq!(m.right, None, "again closes");
        both(&mut m, Action::ToggleRightPanel);
        assert_eq!(m.right, Some(Pane::Queue), "reopens on the tab last shown");
        both(&mut m, Action::NowPlaying);
        assert!(m.now_playing);
        both(&mut m, Action::NowPlaying);
        assert!(!m.now_playing);
        both(&mut m, Action::Find);
        assert!(m.filter);
        both(&mut m, Action::Find);
        assert!(!m.filter);
        both(&mut m, Action::Next);
        both(&mut m, Action::Next);
        both(&mut m, Action::Previous);
        assert_eq!((m.song, m.dir), (1, -1));
        // Local only: a plain toggle opens Up Next.
        let mut m = tour::Mini::default();
        m.apply(Action::ToggleRightPanel, true);
        assert_eq!(m.right, Some(Pane::Queue));
    }

    /// Back goes to the card before, then to the step before; Next always works.
    #[test]
    fn next_and_back_move_through_the_cards() {
        let mut s = at_tour(true);
        next(&mut s);
        next(&mut s);
        assert_eq!(tour_of(&s).card, 2);
        back(&mut s);
        assert_eq!(tour_of(&s).card, 1);
        assert!(s.ui.onboarding.as_ref().unwrap().back, "slides the other way");
        back(&mut s);
        back(&mut s);
        assert_eq!(step(&s), Some(Step::Folders));
    }

    /// A key event for a "mod" chord (Cmd on macOS, Ctrl elsewhere).
    fn chord(key: rune::Key, shift: bool) -> KeyEvent {
        let mac = cfg!(target_os = "macos");
        KeyEvent { key, mods: rune::Mods { cmd: true, ctrl: !mac, shift, ..Default::default() } }
    }

    fn press(rt: &mut rune::Runtime<State>, ev: KeyEvent) {
        rt.key(ev);
        run(rt, 0.2);
    }

    fn tour_now(rt: &mut rune::Runtime<State>) -> (Option<Step>, Tour) {
        let mut out = None;
        rt.update(|s, _| out = Some((step(s), tour_of(s))));
        out.unwrap()
    }

    /// The running first run, on the tour's first card (local files only, so no sign-in step).
    fn tour_window(reduced: bool) -> rune::Runtime<State> {
        let mut rt = window(reduced);
        click(&mut rt, "onb/start");
        click(&mut rt, "onb/choice/local");
        key(&mut rt, rune::Key::Enter);
        assert_eq!(now(&mut rt).0, Some(Step::Tour));
        rt
    }

    /// In a headless window: each card's real chord tries it, twice, and the card stays (the
    /// effect toggles); Next or Return moves on. The app's own shortcuts don't run meanwhile
    /// (Space plays nothing, Now Playing stays shut). With and without Reduce Motion.
    #[test]
    fn pressing_each_card_s_shortcut_toggles_and_next_moves_on() {
        use rune::Key;
        type Probe = fn(&tour::Mini) -> i32;
        for reduced in [false, true] {
            let mut rt = tour_window(reduced);
            rt.update(|s, _| {
                s.playback.local = true;
                s.playback.state = crate::bridge::PlayState::Playing;
            });
            let presses: [(KeyEvent, &str, Probe, [i32; 2]); 7] = [
                (KeyEvent { key: Key::Text(" ".into()), mods: Default::default() }, "Space", |m| m.playing as i32, [1, 0]),
                (chord(Key::Command('b'), false), "sidebar", |m| m.folded as i32, [1, 0]),
                (chord(Key::Command('b'), true), "right sidebar", |m| m.right.is_some() as i32, [1, 0]),
                (chord(Key::Command('u'), false), "Up Next", |m| m.right.is_some() as i32, [1, 0]),
                (chord(Key::Command('f'), true), "Now Playing", |m| m.now_playing as i32, [1, 0]),
                (chord(Key::Command('f'), false), "Find", |m| m.filter as i32, [1, 0]),
                (chord(Key::Right, false), "next song", |m| m.song, [1, 2]),
            ];
            for (n, (ev, what, probe, want)) in presses.into_iter().enumerate() {
                assert_eq!(tour_now(&mut rt).1.card, n, "on card {n} before {what} (reduced: {reduced})");
                assert!(rt.presented("onb/tour/tick").is_none(), "no tick before the first try");
                for (k, w) in want.into_iter().enumerate() {
                    press(&mut rt, ev.clone());
                    let t = tour_now(&mut rt).1;
                    assert_eq!(probe(&t.mini), w, "{what}, press {} (reduced: {reduced})", k + 1);
                    assert!(t.hit.is_some() && t.lit, "{what} flashed its keycap");
                    assert!(rt.presented("onb/tour/tick").is_some(), "the tick shows for {what}");
                    run(&mut rt, 0.8);
                    let t = tour_now(&mut rt).1;
                    assert_eq!((t.card, t.lit), (n, false), "{what} stayed on its card, cap back (reduced: {reduced})");
                    assert!(rt.presented("onb/tour/tick").is_some(), "the tick stays");
                }
                // Next by click on the odd cards, Return on the even ones.
                if n % 2 == 1 {
                    click(&mut rt, "onb/tour/next");
                } else {
                    key(&mut rt, Key::Enter);
                }
                run(&mut rt, 0.3);
                assert_eq!(tour_now(&mut rt).1.card, n + 1, "Next moved on from {what} (reduced: {reduced})");
            }
            // The closing card: Ctrl+2 twice lights its row and stays; Finish ends the tour.
            for _ in 0..2 {
                press(&mut rt, chord(Key::Command('2'), false));
            }
            run(&mut rt, 1.0);
            let (st, t) = tour_now(&mut rt);
            assert_eq!((st, t.card, t.mini.slot), (Some(Step::Tour), 7, Some(2)));
            rt.update(|s, _| {
                assert!(s.local_sent.borrow().is_empty(), "Space didn't pause the app");
                assert!(!s.ui.now_playing, "the app's Now Playing stayed shut");
                assert_eq!(s.route, Route::LocalFiles, "Ctrl+2 didn't open a page behind");
            });
            click(&mut rt, "onb/tour/next");
            assert_eq!(now(&mut rt).0, Some(Step::Done));
        }
    }

    /// Next and Back by click; a focused button doesn't take Space from the tour; another card's
    /// shortcut does nothing; Esc skips from the tour.
    #[test]
    fn next_back_and_escape_in_a_window() {
        use rune::Key;
        let mut rt = tour_window(false);
        click(&mut rt, "onb/tour/next");
        assert_eq!(tour_now(&mut rt).1.card, 1);
        click(&mut rt, "onb/back");
        assert_eq!(tour_now(&mut rt).1.card, 0);
        assert!(rt.has_focus(), "the clicked Back keeps the focus (the case this guards)");
        press(&mut rt, KeyEvent { key: Key::Text(" ".into()), mods: Default::default() });
        let (st, t) = tour_now(&mut rt);
        assert_eq!((st, t.card, t.hit), (Some(Step::Tour), 0, Some(Action::PlayPause)), "Space tried the card, not Back");
        run(&mut rt, 0.8);
        assert_eq!(tour_now(&mut rt).1.card, 0, "and the card stays");
        click(&mut rt, "onb/tour/next");
        // Card 1 (the sidebar): the right sidebar's chord isn't this card's.
        press(&mut rt, chord(Key::Command('b'), true));
        assert_eq!(tour_now(&mut rt).1.hit, None);
        key(&mut rt, Key::Escape);
        run(&mut rt, 1.5);
        let (st, _, route) = now(&mut rt);
        assert_eq!(st, None, "Esc skipped");
        assert_eq!(route, Route::LocalFiles);
        rt.update(|s, _| assert_eq!(s.settings.onboarded, Some(TOUR_VERSION)));
    }

    /// The cards show the user's own chords, in Rune's keycaps for the platform, and a rebound
    /// key is the one that tries the card.
    #[test]
    fn rebound_shortcuts_show_and_work() {
        use rune::Key;
        let mut rt = window(false);
        rt.update(|s, _| {
            s.set_keybind("toggle_sidebar", "alt+s", "mod+b");
            s.set_keybind("play_pause", "", "space");
        });
        click(&mut rt, "onb/start");
        click(&mut rt, "onb/choice/local");
        key(&mut rt, Key::Enter);
        let caps = |rt: &rune::Runtime<State>, key: &str| rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).and_then(|e| e.label);
        let alt = if cfg!(target_os = "macos") { "Option S" } else { "Alt S" };
        assert_eq!(caps(&rt, "onb/tour/caps/toggle_sidebar").as_deref(), Some(alt), "the first card is the sidebar's, on Alt+S");
        assert!(rt.presented("onb/tour/caps/play_pause").is_none());
        press(&mut rt, chord(Key::Command('b'), false));
        assert_eq!(tour_now(&mut rt).1.hit, None, "the old chord does nothing");
        // (A Mac types "ß" for ⌥S; Rune matches it by the key's own "s", which a headless run
        // doesn't report, so the event carries the "s".)
        press(&mut rt, KeyEvent { key: Key::Text("s".into()), mods: rune::Mods { alt: true, ..Default::default() } });
        assert_eq!(tour_now(&mut rt).1.hit, Some(Action::ToggleSidebar), "Alt+S tried it");
        if cfg!(target_os = "linux") {
            // A hit never moves the tour on by itself: Next shows the right sidebar's card.
            click(&mut rt, "onb/tour/next");
            run(&mut rt, 0.8);
            assert_eq!(caps(&rt, "onb/tour/caps/toggle_right_panel").as_deref(), Some("Ctrl Shift B"));
        }
    }

    /// Settings → General → Show again in a window: Welcome, the tour, Done; nothing else.
    #[test]
    fn show_again_in_a_window() {
        let mut s = State::offline();
        s.settings.onboarded = Some(TOUR_VERSION);
        let mut rt = rune::Runtime::new(s, crate::ui::view, (1360.0, 860.0), rune::Env { system_dark: true, scale: 1.0 });
        rt.on_key = Some(crate::ui::on_key);
        rt.poll = Some(crate::ui::poll);
        rt.update(|s, _| super::super::settings::open_tab(s, super::super::settings::Tab::General));
        run(&mut rt, 0.6);
        // General is taller than the panel: scroll to the row first, as a person would.
        rt.update(|_, tx| tx.scroll_center(crate::ui::settings_search::Row::Tour.key()));
        run(&mut rt, 0.6);
        click(&mut rt, "settings/tour");
        run(&mut rt, 1.5);
        assert_eq!(now(&mut rt).0, Some(Step::Welcome));
        key(&mut rt, rune::Key::Enter);
        assert_eq!(now(&mut rt).0, Some(Step::Tour));
        assert!(rt.presented("onb/tour/mini").is_some());
        let dots = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("onb/dots")).and_then(|e| e.label);
        assert_eq!(dots.as_deref(), Some("Step 2 of 3"));
    }

    /// The app's banner and Settings → Account → Sign in open the guided step on its own (not
    /// the bridge's full window); `--login` keeps the full window (debug).
    #[test]
    fn sign_in_buttons_open_the_guided_step() {
        let signed_out = || {
            let mut s = State::offline();
            s.settings.onboarded = Some(TOUR_VERSION);
            s.session.bridge_up = true;
            s.session.authorized = false;
            s
        };
        let open_window = |s: State, settings: bool| {
            let mut rt = rune::Runtime::new(s, crate::ui::view, (1360.0, 860.0), rune::Env { system_dark: true, scale: 1.0 });
            rt.on_key = Some(crate::ui::on_key);
            rt.poll = Some(crate::ui::poll);
            if settings {
                rt.update(|s, _| super::super::settings::open_tab(s, super::super::settings::Tab::Account));
            }
            run(&mut rt, 0.6);
            rt
        };
        for (button, settings) in [("sign-in", false), ("settings/sign-in", true)] {
            let mut rt = open_window(signed_out(), settings);
            click(&mut rt, button);
            rt.update(|s, _| {
                assert_eq!(s.ui.onboarding.as_ref().map(|o| (o.mode, o.step)), Some((Mode::SignIn, Step::SignIn)), "{button}");
                assert!(!s.session.login_window, "{button}: no full window");
                assert!(s.ui.settings.open.is_none());
            });
            run(&mut rt, 1.0);
            assert!(rt.presented("onb/signin/open").is_some(), "{button}: the lead-in shows");
        }
        // `--login`: the full window, as before.
        let mut s = signed_out();
        s.session.debug_login = true;
        let mut rt = open_window(s, false);
        click(&mut rt, "sign-in");
        rt.update(|s, _| {
            assert!(s.ui.onboarding.is_none());
            assert!(s.session.login_window);
        });
    }

    // ------------------------------------------------------------ Reduce Motion

    /// Frames for `secs`, asserting nothing moves: under Reduce Motion every element sits on
    /// its layout goal (no slide, spring or entrance offset), keeps its scale, and nothing
    /// turns (the swirl, the spinners). Only opacity may change (fades). Every other frame.
    fn still(rt: &mut rune::Runtime<State>, secs: f32, what: &str) {
        // Scale and turn per element (`moving()` would also count a presence fade).
        let poses = |rt: &rune::Runtime<State>| -> std::collections::HashMap<String, (f32, f32)> {
            rt.elements()
                .into_iter()
                .map(|e| {
                    let turn = rt.presented_pose(&e.path).map_or(0.0, |p| p.rotation());
                    (e.path, (e.scale, turn))
                })
                .collect()
        };
        let mut last = poses(rt);
        let mut rects: std::collections::HashMap<String, [f32; 4]> = rt.elements().into_iter().map(|e| (e.path, e.rect)).collect();
        for i in 0..(secs * 60.0) as usize {
            rt.frame(1.0 / 60.0);
            // The onboarding layer, and the app's root folded behind it (the app's own live
            // values, the seek bar's fill, aren't the onboarding's motion).
            let ours = |p: &str| p.starts_with("window/onboarding") || p == "window/app-root";
            for e in rt.elements().into_iter().filter(|e| ours(&e.path)) {
                // A leaving element holds where it was (its goal is gone); one staying sits on it.
                // (The frame that took the input may re-lay it out once: a jump, not a motion.)
                let goal = match e.exiting {
                    true if i == 0 => e.rect,
                    true => rects.get(&e.path).copied().unwrap_or(e.rect),
                    false => e.target,
                };
                let off = (0..4).map(|k| (e.rect[k] - goal[k]).abs()).fold(0.0, f32::max);
                assert!(off < 0.5, "{what}: {} is off its place by {off} at frame {i} (exiting: {})", e.path, e.exiting);
            }
            rects = rt.elements().into_iter().map(|e| (e.path, e.rect)).collect();
            let now = poses(rt);
            for (path, (sc, turn)) in &now {
                if let Some((sc0, turn0)) = last.get(path) {
                    assert!((sc - sc0).abs() < 1e-3, "{what}: {path} scales {sc0} -> {sc} at frame {i}");
                    assert!((turn - turn0).abs() < 1e-3, "{what}: {path} turns {turn0} -> {turn} at frame {i}");
                }
            }
            last = now;
        }
    }

    fn scene_window(name: &str) -> rune::Runtime<State> {
        let mut s = State::offline();
        (crate::scenes::SCENES.iter().find(|(n, _)| *n == name).unwrap_or_else(|| panic!("scene {name}")).1)(&mut s);
        let mut rt = rune::Runtime::new(s, crate::ui::view, (800.0, 600.0), rune::Env { system_dark: false, scale: 1.0 });
        rt.set_system_reduced_motion(true);
        rt.on_key = Some(crate::ui::on_key);
        rt.poll = Some(crate::ui::poll);
        rt.frame(0.0);
        rt
    }

    /// Reduce Motion, every step: the whole first run (twist-in, Welcome, Choice, Folders, every
    /// tour card with its key pressed twice, Back, Finish, Done, the fade into the app) and each
    /// sign-in state, entered and left, with nothing sliding, springing, scaling or turning.
    #[test]
    fn reduce_motion_moves_nothing_in_any_step() {
        use rune::Key;
        let mut rt = scene_window("onboarding");
        still(&mut rt, 1.5, "twist-in");
        click(&mut rt, "onb/start");
        still(&mut rt, 0.6, "Welcome to Choice");
        click(&mut rt, "onb/choice/local");
        still(&mut rt, 0.6, "Choice to Folders");
        click(&mut rt, "onb/back");
        still(&mut rt, 0.6, "Back to Choice");
        click(&mut rt, "onb/choice/local");
        key(&mut rt, Key::Enter);
        still(&mut rt, 0.6, "Folders to the tour");
        assert_eq!(now(&mut rt).0, Some(Step::Tour));
        let presses = [
            KeyEvent { key: Key::Text(" ".into()), mods: Default::default() },
            chord(Key::Command('b'), false),
            chord(Key::Command('b'), true),
            chord(Key::Command('u'), false),
            chord(Key::Command('f'), true),
            chord(Key::Command('f'), false),
            chord(Key::Right, false),
            chord(Key::Command(','), false),
        ];
        for (n, ev) in presses.into_iter().enumerate() {
            for k in 0..2 {
                rt.key(ev.clone());
                still(&mut rt, 0.6, &format!("tour card {n}, press {k}"));
            }
            if n == 3 {
                click(&mut rt, "onb/back");
                still(&mut rt, 0.6, "tour Back");
                key(&mut rt, Key::Enter);
            }
            key(&mut rt, Key::Enter);
            still(&mut rt, 0.6, &format!("tour card {n} to the next"));
        }
        assert_eq!(now(&mut rt).0, Some(Step::Done), "Finish");
        still(&mut rt, 2.0, "Done into the app");
        assert_eq!(now(&mut rt).0, None, "faded into the app");

        for name in ["onboarding-signin-starting", "onboarding-signin-lead", "onboarding-signin-waiting",
            "onboarding-signin-closed", "onboarding-signin-done", "onboarding-signin-error", "onboarding-signin-only",
            "onboarding-folders-scanning", "onboarding-choice-picked", "onboarding-replay"]
        {
            let mut rt = scene_window(name);
            still(&mut rt, 1.0, name);
            key(&mut rt, Key::Enter);
            still(&mut rt, 0.6, &format!("{name}, Return"));
            if rt.presented("onb/back").is_some() {
                click(&mut rt, "onb/back");
                still(&mut rt, 0.6, &format!("{name}, Back"));
            }
        }
    }

    /// Every step: Tab and Shift+Tab visit its controls in reading order (the step's own,
    /// primary first, then Back and Skip) and wrap; each has a name for screen readers; the
    /// layer is a named dialog and the swirl is hidden from them.
    #[test]
    fn tab_visits_every_control_in_order_and_each_has_a_name() {
        use rune::Key;
        fn focus(rt: &rune::Runtime<State>) -> Option<String> {
            rt.elements().into_iter().find(|e| e.focused).map(|e| e.key.unwrap_or(e.path))
        }
        fn node<'a>(v: &'a serde_json::Value, key: &str) -> Option<&'a serde_json::Value> {
            if v["key"] == key {
                return Some(v);
            }
            v["children"].as_array()?.iter().find_map(|c| node(c, key))
        }
        let folder = |p: &str| format!("onb/folder/{p}");
        // The scene's first folder is ~/Music (`scenes::home_dir`).
        let music = crate::scenes::home_dir().join("Music").display().to_string();
        let cases: Vec<(&str, Vec<String>)> = vec![
            ("onboarding-welcome", vec!["onb/start".into(), "onb/skip".into()]),
            ("onboarding-choice", ["onb/choice/apple", "onb/choice/local", "onb/back", "onb/skip"].map(String::from).to_vec()),
            ("onboarding-folders", vec![folder(&music), folder("/mnt/media/flac"), "onb/folders/add".into(),
                "onb/folders/go".into(), "onb/back".into(), "onb/skip".into()]),
            ("onboarding-signin-lead", ["onb/signin/open", "onb/signin/not-now", "onb/back", "onb/skip"].map(String::from).to_vec()),
            ("onboarding-signin-waiting", ["onb/signin/again", "onb/signin/cancel", "onb/back", "onb/skip"].map(String::from).to_vec()),
            ("onboarding-signin-closed", ["onb/signin/retry", "onb/signin/not-now", "onb/back", "onb/skip"].map(String::from).to_vec()),
            ("onboarding-signin-error", ["onb/signin/restart", "onb/signin/local", "onb/back", "onb/skip"].map(String::from).to_vec()),
            ("onboarding-tour-sidebar", ["onb/tour/next", "onb/back", "onb/skip"].map(String::from).to_vec()),
            ("onboarding-tour-more", ["onb/tour/next", "onb/back", "onb/skip"].map(String::from).to_vec()),
        ];
        for (scene, want) in cases {
            let mut rt = scene_window(scene);
            run(&mut rt, 0.5);
            let tree = rt.accessibility_json();
            let layer = node(&tree, "onboarding").expect("the layer");
            assert_eq!(layer["role"], "dialog", "{scene}: the layer is a dialog");
            assert_eq!(layer["name"], "Welcome to Uzumaki", "{scene}");
            assert!(node(&tree, "onb/spin").is_none(), "{scene}: the swirl is decorative");
            let mut seen = vec![];
            for _ in 0..want.len() {
                key(&mut rt, Key::Tab);
                seen.push(focus(&rt).unwrap_or_else(|| panic!("{scene}: Tab focused nothing")));
            }
            // Where Tab starts depends on the focus the step gives; the cycle must be `want`'s.
            let at = want.iter().position(|k| *k == seen[0]).unwrap_or_else(|| panic!("{scene}: {} isn't expected", seen[0]));
            let cycle: Vec<String> = want[at..].iter().chain(&want[..at]).cloned().collect();
            assert_eq!(seen, cycle, "{scene}: Tab order");
            rt.key(rune::KeyEvent { key: Key::Tab, mods: rune::Mods { shift: true, ..Default::default() } });
            run(&mut rt, 0.3);
            let back = &cycle[(2 * cycle.len() - 2) % cycle.len()];
            assert_eq!(focus(&rt).as_ref(), Some(back), "{scene}: Shift+Tab goes back one");
            let tree = rt.accessibility_json();
            for k in &want {
                let n = node(&tree, k).unwrap_or_else(|| panic!("{scene}: {k} in the accessibility tree"));
                assert!(n["name"].as_str().is_some_and(|s| !s.trim().is_empty()), "{scene}: {k} has a name");
            }
        }
        // Keycaps read as words.
        let mut rt = scene_window("onboarding-tour-right");
        run(&mut rt, 0.3);
        let tree = rt.accessibility_json();
        let caps = node(&tree, "onb/tour/caps/toggle_right_panel").expect("the card's keycaps");
        let want = if cfg!(target_os = "macos") { "Shift Command B" } else { "Ctrl Shift B" };
        assert_eq!(caps["name"], want);
    }
}
