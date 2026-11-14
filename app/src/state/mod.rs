//! App state, navigation and the one place bridge replies/events change it (`State::apply`).
//!
//! Threading: the bridge reader thread parses JSON into a `Msg` and hands it to the UI thread with
//! `AppHandle::update_if(|s, tx| s.apply(msg, tx))`. Handlers in the view call the action methods
//! below, which mutate state and send bridge commands. In headless runs (`--snap`, scenes) there is
//! no bridge: actions only change state, and scenes fill in fake data.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use rune::{AppHandle, Tx};

use crate::art::Art;
use crate::bridge::{Bridge, Engine, Event, PlayState};
use crate::cache::{self, Cache};
use crate::local::LocalPlayer;
use crate::lyrics::Lyrics;
use crate::models::{Artist, Collection, CollectionKind, Page, SearchResults, Shelf, Song, Station, api};
use crate::settings::{IconVariant, Pin, PinKind, Settings, StartPage};
use crate::tray::TrayCtl;

mod actions;
mod artist;
pub mod autoplay;
pub use artist::More;
pub mod ctl;
mod edits;
mod favourites;
mod library;
mod local;
mod lyrics;
pub mod motion;
mod playlist_edit;
mod playlists;
mod prefs;
pub mod queue;
mod resume;
mod signin;
mod source;
mod update;

pub use self::edits::in_library;
pub use self::local::{LocalLibrary, LocalWhat};
pub use self::prefs::{CacheKind, CacheSizes};
pub use self::queue::{PlayQueue, Repeat, StationRun};
pub use self::source::local_only_answer;

use self::library::append_page;

pub type Handle = AppHandle<State>;

/// Stop paging a library list after this many items (step 2; real paging/virtualisation later).
const LIBRARY_CAP: usize = 10_000;

/// How long after the user changes the volume MusicKit's volume events are taken as echoes.
const VOLUME_ECHO: Duration = Duration::from_millis(500);

/// How long to ignore stray play states after asking for a new song.
const STARTING_GRACE: Duration = Duration::from_secs(15);

/// A second click on the same row within this window is the same request (double-click).
const PLAY_DEBOUNCE: Duration = Duration::from_millis(1500);

/// Songs Apple can't find, skipped in a row, before playback stops (a list of only those).
const MAX_SKIPS: u32 = 5;

/// Stop paging an album/playlist track list after this many tracks.
pub const TRACKS_CAP: usize = 1000;

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Route {
    Home,
    Library(LibraryTab),
    Search,
    Collection { kind: CollectionKind, id: String },
    /// Browse: Apple's editorial shelves.
    New,
    Radio,
    RecentlyAdded,
    Artists,
    Artist { id: String, name: String },
    /// An artist page's shelf in full ("See All"), newest first.
    ArtistShelf { id: String, name: String, shelf: crate::artist::Shelf },
    MadeForYou,
    /// Songs from the music folders (Settings → Local files). In local only mode: Songs.
    LocalFiles,
    /// Local only mode: the music folders' albums (an album opens as `Collection` with a
    /// `local/view.rs` id, an artist as `Artist`).
    LocalAlbums,
    LocalArtists,
    /// Local only mode: the music folders as on disk; `None` lists the music folders.
    Folders { path: Option<std::path::PathBuf> },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum LibraryTab {
    #[default]
    Albums,
    Playlists,
    Songs,
}

/// One request's lifecycle.
#[derive(Clone, Debug, PartialEq, Default)]
pub enum Load<T> {
    #[default]
    Idle,
    Loading,
    Ready(T),
    Failed(String),
}

impl<T> Load<T> {
    pub fn ready(&self) -> Option<&T> {
        match self {
            Load::Ready(t) => Some(t),
            _ => None,
        }
    }
}

/// A library list that loads page after page in the background.
#[derive(Clone, Debug, PartialEq)]
pub struct Paged<T> {
    pub items: Vec<T>,
    /// Idle → Loading (more pages coming) → Ready(()) when done, or Failed.
    pub load: Load<()>,
    /// A fresh copy is being fetched behind the cached `items` (which stay shown, `load` stays
    /// Ready). Nothing to draw differently; exposed for a future "updating" hint.
    pub refreshing: bool,
    /// `items` came from the disk cache and haven't been checked against Apple this session.
    stale: bool,
    /// Pages of the fresh copy, swapped into `items` when the last one arrives.
    buf: Vec<T>,
    /// How long the whole list is, from Apple's first page (`meta.total`); for progress.
    pub total: Option<usize>,
}

impl<T> Default for Paged<T> {
    fn default() -> Self {
        Self { items: Vec::new(), load: Load::Idle, refreshing: false, stale: false, buf: Vec::new(), total: None }
    }
}

impl<T> Paged<T> {
    /// A fully loaded list.
    pub fn ready(items: Vec<T>) -> Self {
        Self { items, load: Load::Ready(()), ..Self::default() }
    }

    /// Shows `items` from the disk cache, unless something newer is already loading.
    fn seed(&mut self, items: Option<Vec<T>>) {
        if let (Load::Idle, Some(items)) = (&self.load, items) {
            self.items = items;
            self.load = Load::Ready(());
            self.stale = true;
        }
    }

    /// Loaded this session (not just read from disk) and not being replaced.
    fn is_fresh(&self) -> bool {
        self.load == Load::Ready(()) && !self.stale && !self.refreshing
    }
}

/// Where a detail page stands against the disk cache.
#[derive(Clone, Copy, Debug, PartialEq)]
enum CollCache {
    /// The cache file is being read on a background thread.
    Reading,
    /// Nothing usable on disk.
    Missing,
    /// The cached copy (saved at this time) is shown; not yet checked against Apple.
    Cached(u64),
    /// Checked (or fetched) this session.
    Verified,
}

/// Where an album or playlist starts playing.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Start {
    At(usize),
    /// From a random playable song, the rest shuffled (the Shuffle button).
    Shuffle,
}

/// A Play waiting for its album or playlist to load (`State::load_then_play`).
#[derive(Clone, Debug, PartialEq)]
struct PendingPlay {
    id: String,
    title: String,
    start: Start,
}

/// The list playing now, while its later pages of songs load: `len` songs of it are queued.
#[derive(Clone, Debug, PartialEq)]
struct Growing {
    id: String,
    title: String,
    len: usize,
}

/// Cached catalog pages younger than this are used without asking Apple again.
const COLLECTION_TTL_SECS: u64 = 24 * 3600;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Session {
    /// The page's MusicKit instance is up (bridge sent `ready`).
    pub bridge_up: bool,
    pub authorized: bool,
    pub storefront: Option<String>,
    /// Fatal: Widevine failed or the Electron process exited.
    pub error: Option<String>,
    /// The Apple sign-in window is showing.
    pub login_window: bool,
    /// Sign out was asked for and the bridge hasn't answered yet.
    pub signing_out: bool,
    /// The onboarding chose Apple Music before the bridge was up: open the sign-in window when
    /// it is (unless already signed in).
    pub login_after_ready: bool,
    /// `/v1/me/storefront` was asked since the start or the sign-in (once each).
    pub storefront_asked: bool,
    /// The account's storefront as Apple answered it (`/v1/me/storefront`) since the start or the
    /// sign-in; the page's MusicKit is kept on it.
    pub account_storefront: Option<String>,
    /// The storefront whose name was asked for this session.
    pub storefront_named: Option<String>,
    /// Started with `--login`: Sign in (banner, Settings) opens the bridge's full window, as
    /// before the guided step (a debug path).
    pub debug_login: bool,
    /// Bumped whenever the account's data is dropped (sign out, sign in): replies about the
    /// account asked before that are stale (`Msg::Account`).
    pub account: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Playback {
    pub song: Option<Song>,
    pub state: PlayState,
    /// Seconds.
    pub position: f64,
    pub duration: f64,
    /// 0..1, as MusicKit reports it.
    pub volume: f64,
    /// While the seek bar is dragged: the fraction (0..1) under the pointer. Commit with `seek_to`.
    pub seek_preview: Option<f64>,
    pub shuffle: bool,
    pub repeat: Repeat,
    /// When `position` was last set, to extrapolate between time events (smooth lyrics).
    pub position_at: Option<Instant>,
    /// Seconds the extrapolated clock is ahead of the player (negative: behind), worked off
    /// over `SLEW_SECS` by running the clock a little slower (or faster) instead of jumping, so
    /// the lyrics never step back on a late or coarse time event. See `apply_time`.
    pub slew: f64,
    /// A seek we sent and when: until the player reports a time near it, reports from before
    /// it took are ignored (see `apply_time`).
    pub seek_target: Option<(f64, Instant)>,
    /// The in-process player (local files) is in charge, not the bridge: its events drive
    /// this struct and the bridge's playback events are ignored.
    pub local: bool,
}

/// How long the playback clock takes to work off a difference from the player's reported time.
const SLEW_SECS: f64 = 1.0;
/// Differences from the reported time beyond this are seeks: the clock jumps.
const MAX_SLEW: f64 = 1.0;
/// After a seek, a report this close to the target (whole-second reports included) confirms it.
const SEEK_SLACK: f64 = 1.5;
/// Seconds after a seek before reports far from its target are believed again (a seek the
/// player refused or ignored).
const SEEK_CONFIRM: f64 = 3.0;
/// How long before a station song's end (seconds) a song queued by hand takes over
/// (`State::station_handover`). Cuts the station song this much short.
const STATION_HANDOFF: f64 = 1.0;
/// Time reports come late and with jitter: the hand-over window is at least one report
/// interval plus this, so a report always lands in it (the `NowPlaying` safety net covers a miss).
const TICK_JITTER: f64 = 0.25;

thread_local! {
    /// `State::landing` made the coming frame's commit instant (see `take_landed`).
    static LANDED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Whether data landed (`State::landing`) since the last call: the frame now polling commits
/// instantly, so any glide asked for in it jumps. Taken once per frame by `ui::poll`.
pub fn take_landed() -> bool {
    LANDED.replace(false)
}

impl Playback {
    /// The position now: the last reported one plus the time since, while playing, minus the
    /// part of `slew` worked off so far. Never decreases between time events.
    pub fn position_now(&self) -> f64 {
        match (self.state, self.position_at) {
            (PlayState::Playing, Some(at)) => {
                let e = crate::clock::since(at).as_secs_f64();
                let t = self.position + e - self.slew * (e / SLEW_SECS).min(1.0);
                t.min(self.duration.max(self.position))
            }
            _ => self.position,
        }
    }

    /// Changes the play state, restarting the clock from where it stands, so resuming after a
    /// pause doesn't count the paused time as played.
    pub fn set_state(&mut self, state: PlayState) {
        self.position = self.position_now();
        self.position_at = Some(crate::clock::now());
        self.slew = 0.0;
        self.state = state;
    }
}

impl Default for Playback {
    fn default() -> Self {
        Self {
            song: None,
            state: PlayState::None,
            position: 0.0,
            duration: 0.0,
            volume: 1.0,
            seek_preview: None,
            shuffle: false,
            repeat: Repeat::Off,
            position_at: None,
            slew: 0.0,
            seek_target: None,
            local: false,
        }
    }
}

/// Everything that reaches the UI thread from the bridge.
#[derive(Debug)]
pub enum Msg {
    Bridge(Event),
    /// A reply about the signed-in account (a `/v1/me` read, any write), stamped with the
    /// `Session::account` it was asked under: one from before a sign-out (or a sign-in) is
    /// dropped, so a page landing late can't bring the old library back.
    Account { generation: u64, msg: Box<Msg> },
    Shelf { index: usize, result: Result<Vec<Collection>, String> },
    LibraryAlbums(Result<Page<Collection>, String>),
    LibraryPlaylists(Result<Page<Collection>, String>),
    LibrarySongs(Result<Page<Song>, String>),
    /// The cheap "did the library change?" probe: `meta.total` and the newest ids.
    LibraryNewest { songs: bool, result: Result<(Option<u64>, Vec<String>), String> },
    Search { seq: u64, result: Result<SearchResults, String> },
    /// MusicKit refused to start the song queued as `first` (the id we sent).
    QueueFailed { first: String, error: String },
    /// MusicKit holds the resumed song `first`, paused at `at` (`State::prime_resume`).
    ResumePrimed { first: String, at: f64 },
    /// A detail page, with the path of its next page of tracks when there are more.
    Collection { id: String, result: Result<(Collection, Option<String>), String> },
    CollectionTracks { id: String, result: Result<Page<Song>, String> },
    /// The library sync's copy of a playlist (`library::PlaylistSync`): its first page, then
    /// the rest of its tracks.
    SyncPlaylist { id: String, result: Result<(Collection, Option<String>), String> },
    SyncPlaylistTracks { id: String, result: Result<Page<Song>, String> },
    /// Lyrics for a catalog song id; Ok(None) when it has none.
    Lyrics { id: String, result: Result<Option<Lyrics>, String> },
    AddedToPlaylist { id: String, name: String, count: usize, result: Result<(), String> },
    /// Shelves of a browse page (New, Made for You, Radio).
    Browse { page: BrowsePage, result: Result<Vec<Shelf>, String> },
    RecentlyAdded(Result<Page<Collection>, String>),
    LibraryArtists(Result<Page<Artist>, String>),
    ArtistAlbums { id: String, result: Result<Vec<Collection>, String> },
    /// A library artist's catalog id (None: no catalog match). `id` is the route's.
    ArtistCatalog { id: String, result: Result<Option<String>, String> },
    /// A catalog artist's page (`crate::artist`); `full` says whether it asked with the extras.
    ArtistPage { id: String, catalog: String, full: bool, result: Result<Option<crate::artist::ArtistPage>, String> },
    /// A later page of an artist page's top songs or shelf.
    ArtistMore { id: String, what: More, limited: bool, result: Result<crate::artist::ViewPage, String> },
    /// The word-timed lyrics request failed: ask for the line-timed ones.
    LyricsRetry { id: String },
    /// The playing song's verified catalog album for animated artwork.
    MotionSongAlbum { id: String, result: Result<Option<String>, String> },
    /// A catalog album's motion art (`api::album_motion`); Ok(None) when it has none.
    Motion { album: String, result: Result<Option<crate::motion::MotionArt>, String> },
    /// A catalog album's page asked with its motion art: both land.
    AlbumPage { id: String, motion: Option<crate::motion::MotionArt>, result: Result<(Collection, Option<String>), String> },
    /// Go to Album for a library song: its library album id.
    AlbumOf { result: Result<Option<String>, String> },
    /// AutoPlay's station for queue `run` (`state/autoplay.rs`).
    AutoplayStation { run: u64, result: Result<String, String> },
    /// Songs of AutoPlay's station for queue `run`.
    AutoplayTracks { run: u64, result: Result<Vec<Song>, String> },
    /// The bridge's answer to `signOut`.
    SignedOut(Result<(), String>),
    /// Apple's storefront (id, name): a name lookup (`/v1/storefronts/{id}`).
    Storefront(Result<(String, Option<String>), String>),
    /// The account's storefront (id, name): Apple's answer to `/v1/me/storefront`.
    AccountStorefront(Result<(String, Option<String>), String>),
    /// Apple's answer to "Add to Library" (`what`: “Title” or "3 songs").
    AddedToLibrary { what: String, result: Result<(), String> },
    /// Apple's answer to removing a library item (`kind`: songs, albums or playlists).
    RemovedFromLibrary { kind: &'static str, id: String, title: String, result: Result<(), String> },
    /// Apple's answer to removing `count` songs from playlist `id`.
    RemovedFromPlaylist { id: String, name: String, count: usize, result: Result<(), String> },
    /// A page of playlist `id`'s entries (id, type), read before rewriting its list.
    PlaylistEntries { id: String, result: Result<Page<(String, String)>, String> },
    /// Apple's answer to creating a playlist (with `count` songs in it).
    PlaylistCreated { name: String, songs: Vec<Song>, result: Result<Collection, String> },
    /// An event of the local-files player, for the song started with `generation`.
    Local { generation: u64, event: Event },
    /// The local-files index: from disk at startup (`scanning` still true), then after a scan.
    LocalIndex { songs: Vec<Song>, view: Box<crate::local::LocalView>, scanning: bool },
    /// Files looked at so far by the running scan.
    LocalScanProgress(usize),
    /// Apple's answer to a star change (`ids`: the songs' library and catalog ids, or the
    /// album's or playlist's).
    FavouriteSet { ids: Vec<String>, on: bool, what: String, result: Result<(), String> },
    /// `personalRating` lookups: the ids asked (with another id the song is known by) and
    /// (id, favourite) for each one Apple listed.
    FavouriteLookup { asked: Vec<(String, Option<String>)>, result: Result<Vec<(String, bool)>, String> },
    /// A page of the Favourite Songs playlist's tracks.
    FavouriteList(Result<Page<Song>, String>),
    /// Apple's answer to renaming playlist `id` to `new` (`old`: its name and description before).
    PlaylistEdited { id: String, old: (String, Option<String>), new: String, result: Result<(), String> },
}

/// Browse pages whose content is a list of shelves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BrowsePage {
    New,
    MadeForYou,
    LiveRadio,
    PersonalStation,
}

impl From<Event> for Msg {
    fn from(e: Event) -> Self {
        Msg::Bridge(e)
    }
}

/// Titles of the Listen now shelves, in order (index = `Msg::Shelf::index`).
pub const SHELF_TITLES: [&str; 2] = ["Recently played", "Recently added"];

pub struct State {
    /// None in headless runs, in local only mode, and until the bridge is started.
    pub bridge: Option<Bridge<Msg>>,
    /// Starts and stops the bridge (`source.rs`); None in headless runs and most tests.
    pub engine: Option<Engine<Msg>>,
    /// The local-files player; None in headless runs and tests.
    pub local: Option<LocalPlayer>,
    /// Generation of the last song given to `local` (its events carry it; older ones are dropped).
    local_gen: u64,
    /// Songs in the music folders.
    pub local_lib: LocalLibrary,
    /// Commands for the local player, for tests (there is no player to record them).
    #[cfg(test)]
    pub(crate) local_sent: std::cell::RefCell<Vec<String>>,
    pub art: Art,
    /// Background pictures with their effect (Settings → Appearance).
    pub backdrops: crate::backdrop::Backdrops,
    pub session: Session,
    pub route: Route,
    pub back_stack: Vec<Route>,
    /// Listen now shelves, one per `SHELF_TITLES` entry.
    pub shelves: [Load<Shelf>; 2],
    pub library_albums: Paged<Collection>,
    pub library_playlists: Paged<Collection>,
    pub library_songs: Paged<Song>,
    /// Last submitted search.
    pub search_query: String,
    pub search_seq: u64,
    pub search: Load<SearchResults>,
    /// Album/playlist detail pages by id.
    pub collections: HashMap<String, Load<Collection>>,
    /// Disk cache; None in headless runs.
    cache: Option<Cache>,
    handle: Option<Handle>,
    coll_cache: HashMap<String, CollCache>,
    /// Detail pages being re-fetched behind a cached copy; the value holds the fresh copy
    /// (first page of tracks) until the last page arrives.
    coll_refresh: HashMap<String, Option<Collection>>,
    /// Playlists whose track list is being rewritten to remove single copies of a song
    /// (`edits.rs`), by playlist id.
    pl_rewrites: HashMap<String, edits::Rewrite>,
    shelf_stale: [bool; 2],
    /// Paths of requests, for tests (there is no bridge to record them).
    #[cfg(test)]
    pub(crate) sent: std::cell::RefCell<Vec<String>>,
    pub playback: Playback,
    /// The play order (what's playing, Up Next, shuffle). See `queue.rs`.
    pub queue: PlayQueue,
    pub history: queue::History,
    /// Lyrics by catalog song id.
    pub lyrics: HashMap<String, Load<Option<Lyrics>>>,
    /// Lyrics requests in flight and retries (`state/lyrics.rs`).
    lyrics_fetch: HashMap<String, lyrics::Fetch>,
    /// Synced lyrics on screen: the bridge sends time 4×/s and every tick redraws.
    pub fine_time: bool,
    /// Who asks for `fine_time`: full-screen Now Playing, the right sidebar.
    fine_time_np: bool,
    fine_time_panel: bool,
    /// A playlist is being created (the New Playlist dialog waits for Apple's answer).
    pub creating_playlist: bool,
    /// When the user last changed the volume (see `VOLUME_ECHO`).
    pub(crate) volume_set_at: Option<Instant>,
    /// Browse pages.
    pub browse_new: Load<Vec<Shelf>>,
    pub made_for_you: Load<Vec<Shelf>>,
    /// Radio page: [live radio, personal station].
    pub radio: [Load<Vec<Shelf>>; 2],
    pub recently_added: Paged<Collection>,
    pub library_artists: Paged<Artist>,
    pub artist_albums: HashMap<String, Load<Vec<Collection>>>,
    /// Artist pages by route id, kept for the session. `Ready(None)`: no catalog page, so the
    /// page is the old albums grid (`artist_albums`).
    pub artist_pages: HashMap<String, Load<Option<crate::artist::ArtistPage>>>,
    /// Persisted user settings (pins, sidebar, Settings overlay). Saved only when `persist_settings`.
    pub settings: Settings,
    persist_settings: bool,
    /// The system tray icon; None when it's off, in headless runs and in tests.
    pub tray: Option<TrayCtl>,
    /// The OS appearance as the view last saw it (`Cx::system_dark`); None until the first frame
    /// and in headless runs. Feeds App icon → Follow theme with Theme = System.
    system_dark: Option<bool>,
    /// The app icon last applied to the desktop (`sync_app_icon`): icon files, bundle and tray
    /// are only touched when the resolved icon differs from it.
    applied_icon: Option<IconVariant>,
    /// Linux: the tray was started before Follow theme could be resolved (Dark was a guess).
    tray_icon_guessed: bool,
    /// Tests: how many times `sync_app_icon` acted (a resolved icon changed).
    #[cfg(test)]
    icon_applies: u32,
    /// Started with `--hidden` (in the background, like a daemon, for `uzu`): closing the
    /// window only hides it.
    pub background: bool,
    /// The full library download (`sync_library`) is running; `ui::intro` shows its progress.
    pub syncing: bool,
    /// Every library playlist's songs and covers, fetched after the lists.
    pl_sync: library::PlaylistSync,
    /// Bytes on disk per cache (Settings → General); None until measured.
    pub cache_sizes: Option<CacheSizes>,
    /// Last non-fatal error, shown briefly.
    pub toast: Option<String>,
    /// A newer version is out (`update.rs`, `state/update.rs`): the banner atop the content.
    pub update: Option<crate::update::Notice>,
    /// The manual update check and when the last check ran (Settings → General).
    pub update_check: crate::update::CheckStatus,
    /// The hourly timer's bookkeeping (`state/update.rs`).
    update_auto: update::AutoCheck,
    /// Resume where you left off (`state/resume.rs`): the last run's song is shown paused and
    /// hasn't been handed to a player yet. Cleared by Play or any new song.
    pub resume: Option<resume::Pending>,
    /// What `resume.rs` was last given, and when (see `State::resume_changed`).
    resume_out: resume::Published,
    /// Last play request and when, to swallow double-clicks.
    last_play: Option<(String, Instant)>,
    /// A Play or Shuffle of an album or playlist whose songs are loading (`load_then_play`).
    pending_play: Option<PendingPlay>,
    /// The list playing now is still getting its later pages of songs (`grow_queue`).
    growing: Option<Growing>,
    /// AutoPlay's station for the queue playing (`autoplay.rs`).
    autoplay: autoplay::Run,
    /// Script hooks (`hooks.rs`): told after each change of song or play state.
    hooks: crate::hooks::Hooks,
    /// "Notify on song change" (`notify.rs`), fed by the hooks' song detector.
    notifier: crate::notify::Notifier,
    /// Set when we asked for a new song and it hasn't started yet. Meanwhile MusicKit reports
    /// the old queue stopping (nothing playing, paused, …), which must not flicker the player bar.
    starting: Option<Instant>,
    /// Songs skipped in a row because Apple couldn't find them (see `queue_failed`).
    skipped_in_row: u32,
    /// Apple's stars: what is known about songs, albums and playlists.
    favs: favourites::Favourites,
    /// The picture chosen in the New Playlist dialog, for the playlist Apple is creating.
    new_playlist_picture: Option<std::path::PathBuf>,
    /// Animated artwork in Now Playing (`motion.rs`).
    pub motion: motion::MotionState,
    /// View-only state owned by the UI module (text field contents, scroll, …).
    pub ui: crate::ui::UiState,
}

impl State {
    /// Headless / scene state: no bridge, no downloads.
    /// The app's handle (None headless and in tests), for the macOS menu bar.
    #[cfg(target_os = "macos")]
    pub fn app_handle(&self) -> Option<&Handle> {
        self.handle.as_ref()
    }

    pub fn offline() -> Self {
        Self::new(None, None, None)
    }

    pub fn new(bridge: Option<Bridge<Msg>>, local: Option<LocalPlayer>, handle: Option<Handle>) -> Self {
        let persist = handle.is_some(); // real app (not headless/tests): read and write settings
        let settings = if persist { Settings::load() } else { Settings::default() };
        let notifier = if persist { crate::notify::Notifier::for_app() } else { crate::notify::Notifier::off() };
        notifier.set_enabled(settings.notify_song_change);
        let route = match (&settings.start_page, &settings.last_route) {
            (StartPage::Last, Some(r)) if settings.music_source == crate::settings::MusicSource::LocalOnly && !r.needs_apple() => r.clone(),
            _ if settings.music_source == crate::settings::MusicSource::LocalOnly => Route::LocalFiles,
            (StartPage::Last, Some(r)) => r.clone(),
            _ => Route::Home,
        };
        let playback = Playback { shuffle: settings.shuffle, repeat: settings.repeat, ..Playback::default() };
        let mut ui = crate::ui::UiState::default();
        if settings.music_source == crate::settings::MusicSource::LocalOnly {
            ui.search_tab = crate::ui::SearchTab::Local;
        }
        ui.table.apply(settings.table);
        // The tray starts with the resolved icon. Follow theme with Theme = System isn't known
        // until the first frame reports the appearance: Dark until then (`sync_app_icon`).
        let tray_icon = settings.app_icon.resolve(settings.theme, None);
        let tray = match (&handle, settings.tray_icon) {
            (Some(h), true) => TrayCtl::start(h.clone(), tray_icon.unwrap_or(IconVariant::Dark)),
            _ => None,
        };
        let mut s = Self {
            bridge,
            engine: None,
            local,
            local_gen: 0,
            local_lib: LocalLibrary::default(),
            #[cfg(test)]
            local_sent: Default::default(),
            art: Art::new(handle.clone()),
            backdrops: crate::backdrop::Backdrops::new(handle.clone()),
            session: Session::default(),
            route,
            back_stack: Vec::new(),
            shelves: Default::default(),
            library_albums: Paged::default(),
            library_playlists: Paged::default(),
            library_songs: Paged::default(),
            search_query: String::new(),
            search_seq: 0,
            search: Load::Idle,
            collections: HashMap::new(),
            cache: handle.as_ref().map(|_| Cache::default_location()),
            handle,
            coll_cache: HashMap::new(),
            coll_refresh: HashMap::new(),
            pl_rewrites: HashMap::new(),
            shelf_stale: [false; 2],
            #[cfg(test)]
            sent: Default::default(),
            playback,
            toast: None,
            update: None,
            update_check: Default::default(),
            update_auto: Default::default(),
            resume: None,
            resume_out: Default::default(),
            last_play: None,
            pending_play: None,
            growing: None,
            autoplay: autoplay::Run::default(),
            hooks: if persist { crate::hooks::Hooks::for_app() } else { crate::hooks::Hooks::off() },
            notifier,
            starting: None,
            skipped_in_row: 0,
            favs: Default::default(),
            new_playlist_picture: None,
            motion: motion::MotionState::default(),
            ui,
            browse_new: Load::Idle,
            made_for_you: Load::Idle,
            radio: Default::default(),
            recently_added: Paged::default(),
            library_artists: Paged::default(),
            artist_albums: HashMap::new(),
            artist_pages: HashMap::new(),
            settings,
            persist_settings: persist,
            tray,
            background: false,
            syncing: false,
            pl_sync: Default::default(),
            cache_sizes: None,
            queue: PlayQueue::default(),
            history: queue::History::default(),
            lyrics: HashMap::new(),
            lyrics_fetch: HashMap::new(),
            fine_time: false,
            fine_time_np: false,
            fine_time_panel: false,
            creating_playlist: false,
            volume_set_at: None,
            system_dark: None,
            applied_icon: None,
            tray_icon_guessed: tray_icon.is_none(),
            #[cfg(test)]
            icon_applies: 0,
        };
        // The installed icon files (Linux) or the .app bundle's icon (macOS) follow the setting
        // at every start (a reinstall puts the dark one back).
        s.sync_app_icon();
        // The saved storefront, for Settings and links before the bridge is up.
        s.session.storefront = s.settings.storefront.as_ref().map(|f| f.id.clone());
        s
    }

    // ---------------------------------------------------------------- bridge → state

    /// Applies one message. Returns whether anything the view reads changed.
    pub fn apply(&mut self, msg: Msg, tx: &mut Tx) -> bool {
        let time = matches!(msg, Msg::Bridge(Event::Time { .. }) | Msg::Local { event: Event::Time { .. }, .. });
        let changed = self.apply_msg(msg);
        if let (true, Some(song)) = (self.hooks.observe(self.playback.song.as_ref(), self.playback.state), self.playback.song.as_ref()) {
            self.notifier.song_changed(song, self.playback.state);
        }
        self.favourite_lookups();
        self.resume_changed(false);
        if time {
            // A frame for `ui::poll`, which moves the seek bars to the clock: a report that
            // changes it without changing the second (a small jump, or while paused) re-runs no
            // view, and while paused nothing else would draw it.
            tx.wake_after(Duration::ZERO);
        }
        self.landing(tx); // after: signing in may have just started the sync screen
        changed
    }

    /// Data arriving shouldn't animate: commit it instantly. Except while the startup intro
    /// runs: Rune lands every animation of a frame at once when any handler in it calls
    /// `instant()`, so a library page arriving with the intro's next step would make it jump.
    /// Behind the intro the app is hidden, so its lists may as well move on their springs.
    ///
    /// Updates run between frames, so the next frame's commit is instant, `poll`'s scroll
    /// requests included: the synced lyrics hold a line change back one frame then
    /// (`take_landed`), or the glide to it would land as a jump.
    pub fn landing(&self, tx: &mut Tx) {
        if self.ui.intro.is_none() && self.ui.onboarding.is_none() {
            tx.instant();
            LANDED.set(true);
        }
    }

    fn apply_msg(&mut self, msg: Msg) -> bool {
        match msg {
            Msg::Bridge(event) => return self.apply_event(event),
            Msg::Account { generation, msg } => {
                if generation != self.session.account {
                    return false; // asked before a sign-out or sign-in
                }
                return self.apply_msg(*msg);
            }
            Msg::Shelf { index, result } => {
                if let Some(slot) = self.shelves.get_mut(index) {
                    *slot = match result {
                        Ok(items) => Load::Ready(Shelf { title: SHELF_TITLES[index].to_string(), items, songs: vec![], stations: vec![] }),
                        Err(_) if matches!(slot, Load::Ready(_)) => return false, // keep the cached shelf
                        Err(e) => Load::Failed(e),
                    };
                    self.save_shelves();
                }
            }
            Msg::LibraryAlbums(result) => {
                let ok = result.is_ok();
                match append_page(&mut self.library_albums, result) {
                    Some(next) => self.request(api::next(&next, 100), |v| Page::parse(v, Collection::parse), Msg::LibraryAlbums),
                    None if ok => self.save(cache::ALBUMS, self.library_albums.items.clone()),
                    None => {}
                }
            }
            Msg::LibraryPlaylists(result) => {
                let ok = result.is_ok();
                match append_page(&mut self.library_playlists, result) {
                    Some(next) => self.request(api::next(&next, 100), |v| Page::parse(v, Collection::parse), Msg::LibraryPlaylists),
                    None if ok => {
                        self.save(cache::PLAYLISTS, self.library_playlists.items.clone());
                        self.load_route_data(); // cached playlist pages can now be checked
                        self.start_playlist_sync();
                    }
                    None => {}
                }
            }
            Msg::LibrarySongs(result) => {
                let ok = result.is_ok();
                match append_page(&mut self.library_songs, result) {
                    Some(next) => self.request(api::next(&next, 100), |v| Page::parse(v, Song::parse), Msg::LibrarySongs),
                    None if ok => self.save(cache::SONGS, self.library_songs.items.clone()),
                    None => {}
                }
            }
            Msg::LibraryNewest { songs, result } => {
                // A failed probe keeps the cached list as it is.
                let Ok((total, ids)) = result else { return false };
                if songs {
                    // Songs are stored oldest first, so the newest are at the end.
                    let l = &self.library_songs;
                    let same = l.items.len() as u64 == total.unwrap_or(u64::MAX)
                        && cache::same_newest(l.items.iter().rev().map(|x| x.id.as_str()), &ids);
                    if l.load == Load::Ready(()) && !same {
                        self.library_songs.refreshing = true;
                        self.request(api::library_songs(), |v| Page::parse(v, Song::parse), Msg::LibrarySongs);
                    }
                } else {
                    let l = &self.library_albums;
                    let same = l.items.len() as u64 == total.unwrap_or(u64::MAX)
                        && cache::same_newest(l.items.iter().map(|x| x.id.as_str()), &ids);
                    if l.load == Load::Ready(()) && !same {
                        self.library_albums.refreshing = true;
                        self.request(api::library_albums(), |v| Page::parse(v, Collection::parse), Msg::LibraryAlbums);
                    }
                }
                return false;
            }
            Msg::QueueFailed { first, error } => self.queue_failed(&first, &error),
            Msg::ResumePrimed { first, at } => {
                self.resume_primed(&first, at);
                return false;
            }
            Msg::Search { seq, result } => {
                if seq != self.search_seq {
                    return false; // an older search answered late
                }
                self.search = match result {
                    Ok(r) => Load::Ready(r),
                    Err(e) => Load::Failed(e),
                };
            }
            Msg::Collection { id, result } => {
                if let Some(slot) = self.coll_refresh.get_mut(&id) {
                    // Re-fetching behind a cached copy: keep showing it until the last page.
                    match result {
                        Ok((c, Some(next))) => {
                            *slot = Some(c);
                            self.request_tracks(&id, &next);
                        }
                        Ok((c, None)) => self.finish_refresh(id, c),
                        Err(_) => {
                            self.coll_refresh.remove(&id);
                        }
                    }
                    return true;
                }
                let mut more = false;
                let load = match result {
                    Ok((c, next)) => {
                        match next {
                            Some(next) => {
                                more = true;
                                self.request_tracks(&id, &next);
                            }
                            None => self.save_collection(&c),
                        }
                        Load::Ready(c)
                    }
                    Err(e) => Load::Failed(e),
                };
                self.collections.insert(id.clone(), load);
                self.pending_play_landed(&id, more);
            }
            Msg::Browse { page, result } => {
                let load = match result {
                    Ok(shelves) => Load::Ready(shelves),
                    Err(e) => Load::Failed(e),
                };
                match page {
                    BrowsePage::New => self.browse_new = load,
                    BrowsePage::MadeForYou => self.made_for_you = load,
                    BrowsePage::LiveRadio => self.radio[0] = load,
                    BrowsePage::PersonalStation => self.radio[1] = load,
                }
            }
            Msg::RecentlyAdded(result) => {
                if let Some(next) = append_page(&mut self.recently_added, result) {
                    if self.recently_added.items.len() < 200 {
                        self.request(api::next(&next, 25), |v| Page::parse(v, Collection::parse), Msg::RecentlyAdded);
                    } else {
                        self.recently_added.load = Load::Ready(());
                    }
                }
            }
            Msg::LibraryArtists(result) => {
                if let Some(next) = append_page(&mut self.library_artists, result) {
                    self.request(api::next(&next, 100), |v| Page::parse(v, Artist::parse), Msg::LibraryArtists);
                }
            }
            Msg::ArtistAlbums { id, result } => {
                let load = match result {
                    Ok(a) => Load::Ready(a),
                    Err(e) => Load::Failed(e),
                };
                self.artist_albums.insert(id, load);
            }
            Msg::ArtistCatalog { id, result } => self.on_artist_catalog(id, result),
            Msg::ArtistPage { id, catalog, full, result } => self.on_artist_page(id, catalog, full, result),
            Msg::ArtistMore { id, what, limited, result } => self.on_artist_more(id, what, limited, result),
            Msg::Local { generation, event } => return self.apply_local(generation, event),
            Msg::LocalIndex { songs, view, scanning } => {
                self.set_local_index(songs, *view);
                if !scanning && std::mem::take(&mut self.local_lib.again) {
                    self.local_lib.scanning = false;
                    self.rescan_local(); // the folders changed while it ran
                } else {
                    self.local_lib.scanning = scanning;
                }
            }
            Msg::LocalScanProgress(n) => {
                if !self.local_lib.scanning {
                    return false;
                }
                self.local_lib.scanned = n;
            }
            Msg::PlaylistCreated { name, songs, result } => self.playlist_created(name, songs, result),
            Msg::AddedToLibrary { what, result } => self.added_to_library(what, result),
            Msg::RemovedFromLibrary { kind, id, title, result } => self.removed_from_library(kind, id, title, result),
            Msg::RemovedFromPlaylist { id, name, count, result } => self.removed_from_playlist(id, name, count, result),
            Msg::PlaylistEntries { id, result } => self.playlist_entries(id, result),
            Msg::FavouriteSet { ids, on, what, result } => self.favourite_set(ids, on, what, result),
            Msg::FavouriteLookup { asked, result } => return self.favourite_lookup(asked, result),
            Msg::FavouriteList(result) => return self.favourite_list(result),
            Msg::PlaylistEdited { id, old, new, result } => self.playlist_edited(id, old, new, result),
            Msg::LyricsRetry { id } => {
                self.request_lyrics(id, false);
                return false;
            }
            Msg::Storefront(result) => return self.storefront_reply(result),
            Msg::AccountStorefront(result) => return self.account_storefront_reply(result),
            Msg::SignedOut(result) => {
                self.session.signing_out = false;
                match result {
                    Ok(()) => self.signed_out(),
                    Err(e) => self.toast = Some(format!("Couldn't sign out: {e}")),
                }
            }
            Msg::MotionSongAlbum { id, result } => {
                self.motion_song_album_known(id, result);
                return false;
            }
            Msg::Motion { album, result } => {
                self.motion_known(album, result);
                return false; // `poll_motion` takes it from here
            }
            Msg::AlbumPage { id, motion, result } => {
                if result.is_ok() {
                    self.motion_known(id.clone(), Ok(motion));
                }
                return self.apply_msg(Msg::Collection { id, result });
            }
            Msg::AutoplayStation { run, result } => {
                self.autoplay_station(run, result);
                return false;
            }
            Msg::AutoplayTracks { run, result } => self.autoplay_tracks(run, result),
            Msg::AlbumOf { result } => match result {
                Ok(Some(id)) => self.navigate(Route::Collection { kind: CollectionKind::Album, id }),
                Ok(None) => self.toast = Some("Couldn't find the album of this song.".into()),
                Err(e) => self.toast = Some(format!("Couldn't open the album: {e}")),
            },
            Msg::Lyrics { id, result } => self.lyrics_reply(id, result),
            Msg::AddedToPlaylist { id, name, count, result } => match result {
                Ok(()) => {
                    self.toast = Some(match count {
                        1 => format!("Added to “{name}”"),
                        n => format!("Added {n} songs to “{name}”"),
                    });
                    // Its cached page is out of date now.
                    self.collections.remove(&id);
                    self.coll_cache.remove(&id);
                    if let Some(c) = &self.cache {
                        c.forget_collection(&id);
                    }
                }
                Err(e) => self.toast = Some(format!("Couldn't add to “{name}”: {e}")),
            },
            Msg::SyncPlaylist { id, result } => return self.sync_playlist_first(id, result),
            Msg::SyncPlaylistTracks { id, result } => return self.sync_playlist_tracks(id, result),
            Msg::CollectionTracks { id, result } => {
                if let Some(Some(mut c)) = self.coll_refresh.remove(&id) {
                    match result {
                        Ok(page) => {
                            c.tracks.extend(page.items);
                            match page.next {
                                Some(next) if c.tracks.len() < TRACKS_CAP => {
                                    self.request_tracks(&id, &next);
                                    self.coll_refresh.insert(id, Some(c));
                                }
                                _ => self.finish_refresh(id, c),
                            }
                        }
                        Err(_) => {} // the cached copy stays
                    }
                    return true;
                }
                let Some(Load::Ready(c)) = self.collections.get_mut(&id) else { return false };
                match result {
                    Ok(page) => {
                        c.tracks.extend(page.items.iter().cloned());
                        let more = match page.next {
                            Some(next) if c.tracks.len() < TRACKS_CAP => {
                                self.request_tracks(&id, &next);
                                true
                            }
                            _ => {
                                let c = c.clone();
                                self.save_collection(&c);
                                false
                            }
                        };
                        self.grow_queue(&id, &page.items, more);
                    }
                    Err(e) => self.toast = Some(format!("Couldn't load the rest of the tracks: {e}")),
                }
            }
        }
        true
    }

    fn apply_event(&mut self, event: Event) -> bool {
        match event {
            Event::Widevine { ok, error } => {
                if !ok {
                    self.session.error = Some(format!(
                        "Widevine (DRM) could not be installed: {}",
                        error.unwrap_or_else(|| "unknown error".into())
                    ));
                } else {
                    return false;
                }
            }
            Event::Ready { authorized, storefront, volume } => {
                self.session.bridge_up = true;
                if let Some(v) = volume {
                    self.playback.volume = v;
                }
                self.set_authorized(authorized);
                self.storefront_seen(storefront);
                if std::mem::take(&mut self.session.login_after_ready) && !self.session.authorized {
                    self.show_login(true);
                }
                self.load_route_data();
                if crate::ui::intro::syncing(self) {
                    self.sync_library();
                }
                self.reapply_playback_prefs();
                self.resume_bridge_ready();
            }
            Event::Auth { authorized, storefront } => {
                self.set_authorized(authorized);
                if authorized {
                    self.storefront_seen(storefront);
                    self.prime_resume();
                }
            }
            Event::Storefront(id) => self.storefront_seen(Some(id)),
            // A resumed song waits for Play: MusicKit (holding it paused, or not yet) has
            // nothing to say about playback until then.
            Event::NowPlaying(_) | Event::PlayState(_) | Event::Time { .. } if self.resume.is_some() => return false,
            // The local player is in charge: the paused bridge has nothing to say about playback.
            Event::NowPlaying(_) | Event::PlayState(_) | Event::Time { .. } if self.playback.local => return false,
            // Queue replaced or ended: keep showing the last song. The end of the last song of a
            // station one-off hands back to the station (whatever end state MusicKit sends, if any).
            Event::NowPlaying(None) => {
                let starting = self.starting.is_some_and(|t| crate::clock::since(t) < STARTING_GRACE);
                return !starting && self.bridge_queue_emptied();
            }
            Event::NowPlaying(Some(song)) => {
                // The station moved on to a song of its own while songs are queued by hand (the
                // end-of-song hand-over in `station_handover` was missed): the queued song plays
                // at once, and the station's song, never really heard, stays out of the history.
                if self.queue.station_skipped_queue(&song) {
                    if let Some(next) = self.queue.station_take_user() {
                        self.begin_current(next);
                        return true;
                    }
                }
                // Keep our own copy of the same track (the one shown optimistically on click): it
                // has the artwork URL the bar already loaded, so the cover doesn't reload.
                let lap = self.queue.lap_gen;
                self.queue.advance_to(&song);
                self.queue.station_heard(&song);
                self.history.heard(&song);
                // A new lap began (the planned one became current, and the next one was shuffled):
                // tell the bridge the order it should loop into.
                if self.queue.plan_lap(self.playback.repeat) || self.queue.lap_gen != lap {
                    self.sync_upcoming();
                } else {
                    self.autoplay_check(); // fewer songs left: maybe time for AutoPlay's next ones
                }
                self.want_lyrics(&song);
                if let Some(a) = &song.artwork {
                    self.art.prefetch(a, crate::art::NOW_PLAYING_PX);
                }
                for next in self.queue.up_next().take(2) {
                    if let Some(a) = &next.artwork {
                        self.art.prefetch(a, crate::art::NOW_PLAYING_PX);
                    }
                }
                let same = self.playback.song.as_ref().is_some_and(|cur| cur.same_track(&song));
                if !same {
                    self.playback.duration = song.duration;
                    if let Some(t) = &mut self.tray {
                        t.set_song(Some(&song));
                    }
                    self.playback.song = Some(song);
                } else if let (Some(cur), Some(bg)) = (self.playback.song.as_mut(), song.artwork.and_then(|a| a.bg)) {
                    // Keep our artwork URL but take Apple's background colour (Now Playing tint).
                    if let Some(a) = cur.artwork.as_mut() {
                        a.bg.get_or_insert(bg);
                    }
                }
                // The song we started at a position (a resumed song, or a seek) is announced
                // only now: it still starts there, so the bar and the lyrics don't flash 0:00.
                // A new song starts from 0.
                if !(same && self.playback.seek_target.is_some()) {
                    self.playback.position = 0.0;
                    self.playback.seek_target = None;
                }
            }
            Event::PlayState(state) => {
                if state == PlayState::Playing {
                    // A start at a position (Resume where you left off) counts its wait for a
                    // report near the target from when the audio runs, not from the click.
                    if self.starting.take().is_some() {
                        if let Some((_, at)) = &mut self.playback.seek_target {
                            *at = crate::clock::now();
                        }
                    }
                    self.skipped_in_row = 0;
                } else if self.starting.is_some_and(|t| crate::clock::since(t) < STARTING_GRACE) {
                    return false; // the old queue winding down; we already show "loading"
                }
                if self.bridge_queue_ended(state) {
                    return true; // the app took over with the next song (a local file, or a wrap)
                }
                if state == self.playback.state {
                    return false;
                }
                self.playback.set_state(state);
                if let Some(t) = &mut self.tray {
                    t.set_playing(state.is_active());
                }
            }
            Event::Time { current, duration } => {
                self.lyrics_due();
                let redraw = self.apply_time(current, duration);
                return self.station_handover() || redraw;
            }
            Event::Volume(v) => {
                if (v - self.playback.volume).abs() < 0.001 {
                    return false; // the echo of our own slider
                }
                // While the user is changing it (slider, wheel, keys), MusicKit's late echoes
                // of earlier values would pull it back.
                if self.volume_set_at.is_some_and(|t| crate::clock::since(t) < VOLUME_ECHO) {
                    return false;
                }
                self.playback.volume = v;
            }
            Event::Window { visible, closed } => {
                self.session.login_window = visible;
                if closed {
                    crate::ui::onboarding::window_closed(self);
                }
            }
            Event::Error(e) => {
                self.starting = None;
                self.toast = Some(e);
            }
            // Stopped on purpose (`stop_bridge` already did what the app needs).
            Event::Stopped => return false,
            Event::Exited => {
                self.session.bridge_up = false;
                if self.session.error.is_none() {
                    self.session.error = Some("The playback engine (Electron) stopped. See /tmp/uzumaki-bridge.log.".into());
                }
            }
        }
        true
    }

    /// A station plays, songs were queued by hand and the station's song is about to end: the
    /// queued song starts now, before the station picks one of its own (MusicKit can't queue a
    /// song behind a station). Starting it makes it a one-off, so this fires once per song.
    fn station_handover(&mut self) -> bool {
        let p = &self.playback;
        let due = self.queue.station_live()
            && !self.queue.user.is_empty()
            && p.state == PlayState::Playing
            && p.seek_target.is_none()
            && p.duration > 0.0
            && p.position_now() >= p.duration - self.station_handoff_window();
        if !due {
            return false;
        }
        match self.queue.station_take_user() {
            Some(next) => {
                self.begin_current(next);
                true
            }
            None => false,
        }
    }

    /// Seconds before the end at which `station_handover` fires: `STATION_HANDOFF`, widened to
    /// one time-report interval (1 s, or 0.25 s while synced lyrics show) plus jitter.
    fn station_handoff_window(&self) -> f64 {
        let tick = if self.fine_time { 0.25 } else { 1.0 };
        STATION_HANDOFF.max(tick + TICK_JITTER)
    }

    /// A time event (bridge or local player). Returns whether the view must redraw.
    fn apply_time(&mut self, current: f64, duration: f64) -> bool {
        let duration_changed = duration > 0.0 && duration != self.playback.duration;
        if duration > 0.0 {
            self.playback.duration = duration;
        }
        // Right after a seek, reports from before it took still come for a few hundred ms
        // (MusicKit's whole-second time keeps the old position for a moment, and the bridge
        // falls back to it while the audio element disagrees). Following them snapped the seek
        // bar and the lyrics back to where they were, then forward again. The clock keeps
        // running from the target until a report near it arrives, or `SEEK_CONFIRM` passes.
        if let Some((target, at)) = self.playback.seek_target {
            let waited = crate::clock::since(at).as_secs_f64();
            let near = current > target - SEEK_SLACK && current < target + waited + SEEK_SLACK;
            // A seek into the last seconds may end the song before any report: repeat-one in
            // the bridge starts it over (the same song, so no NowPlaying clears the guard).
            let wrapped = target > self.playback.duration - SEEK_SLACK - waited && current < waited + SEEK_SLACK;
            if !near && !wrapped && waited < SEEK_CONFIRM {
                return duration_changed;
            }
            self.playback.seek_target = None;
        }
        let shown = self.playback.position_now().floor();
        // Time events arrive late and with jitter, and MusicKit's own time is whole seconds
        // (the bridge sends the audio element's exact time when it can). Stepping the clock back
        // to a report flipped the lyrics between two lines, so while playing it stays continuous
        // and works the difference off over `SLEW_SECS`. A whole-second report only says the
        // player is somewhere in [n, n + 1). Bigger differences are seeks and jump.
        let est = self.playback.position_now();
        let target = if current.fract() == 0.0 { est.clamp(current, current + 1.0) } else { current };
        let lag = est - target;
        let playing = self.playback.state == PlayState::Playing;
        if playing && self.playback.position_at.is_some() && lag.abs() < MAX_SLEW {
            self.playback.position = est;
            self.playback.slew = lag;
        } else {
            self.playback.position = current;
            self.playback.slew = 0.0;
        }
        self.playback.position_at = Some(crate::clock::now());
        // The seek bars and the lyrics follow the clock on their own (`ui::poll`, which `apply`
        // wakes); the view only re-runs for the second the time labels show.
        self.playback.position_now().floor() != shown || duration_changed
    }

    fn set_authorized(&mut self, authorized: bool) {
        let was = self.session.authorized;
        self.session.authorized = authorized;
        self.save(cache::SESSION, authorized);
        if authorized && self.session.login_window {
            // Signed in (just now, or already when `--login` opened the window): the login
            // window has done its job.
            self.show_login(false);
        }
        if authorized && !was {
            // Signed in just now: every /me list is stale.
            self.clear_personal();
            self.load_route_data();
            // Every list is downloading again: the sync screen covers it (`ui::intro`).
            self.begin_sync();
            // The song on screen may have been skipped while signed out.
            if let Some(song) = self.playback.song.clone() {
                self.want_lyrics(&song);
            }
        } else if !authorized && was {
            // Signed out (or the cache guessed wrong): drop the account's data, in memory and on disk.
            self.drop_account();
        }
    }

}

#[cfg(test)]
mod tests;
