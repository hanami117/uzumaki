//! Where the music comes from: Apple Music (through the bridge) or local files only. The bridge
//! starts when Apple Music is wanted and stops for local only; the Apple sign-in, caches and
//! settings stay either way.

use super::*;
use crate::settings::MusicSource;

impl Route {
    /// Pages that show Apple Music: hidden in local only mode.
    pub fn needs_apple(&self) -> bool {
        match self {
            Route::Search | Route::LocalFiles => false,
            Route::Collection { id, .. } => !crate::local::view::is_album_id(id) && !crate::mixed::is_uzu_id(id),
            Route::Artist { id, .. } => !crate::local::view::is_artist_id(id),
            r => !r.local_library(),
        }
    }

    /// An Uzumaki playlist's page (kept on this computer, in both modes).
    pub fn uzumaki(&self) -> bool {
        matches!(self, Route::Collection { id, .. } if crate::mixed::is_uzu_id(id))
    }

    /// A page of the account's own library (a library album, playlist or artist): it goes
    /// when signing out.
    pub fn personal(&self) -> bool {
        match self {
            Route::Collection { id, .. } => super::library::personal_id(id),
            Route::Artist { id, .. } | Route::ArtistShelf { id, .. } => crate::models::Artist::is_library_id(id),
            _ => false,
        }
    }

    /// Local only mode's own pages (Albums, Artists, Folders, a local album or artist): Apple
    /// mode has Local Files instead.
    pub fn local_library(&self) -> bool {
        match self {
            Route::LocalAlbums | Route::LocalArtists | Route::Folders { .. } => true,
            Route::Collection { id, .. } => crate::local::view::is_album_id(id),
            Route::Artist { id, .. } => crate::local::view::is_artist_id(id),
            _ => false,
        }
    }
}

/// What a `uzu` command that needs Apple Music answers in local only mode.
pub fn local_only_answer(what: &str) -> String {
    format!("Uzumaki is in local only mode, so {what} needs Apple Music. Switch in Settings \u{2192} Account.")
}

/// Crashes this close together are counted toward the automatic restarts' limit.
const CRASH_WINDOW: Duration = Duration::from_secs(10 * 60);
/// Waits before each automatic restart after a crash; past the last, the banner's Restart button.
const REVIVE_AFTER: [Duration; 3] = [Duration::from_secs(1), Duration::from_secs(5), Duration::from_secs(20)];

/// The playback engine's crashes (`bridge_exited`).
#[derive(Debug, Default)]
pub struct Revive {
    crashes: Vec<Instant>,
    /// Bumped by every crash and restart: a timer from an older one does nothing.
    generation: u64,
    /// An automatic restart is waiting for its backoff.
    pub waiting: bool,
}

impl State {
    /// Electron ended on its own (crash, OOM kill, killed by hand). The dead bridge goes, an
    /// Apple song stops where it was (Play picks it up again once a new engine is up, as a
    /// resumed song does), and a new engine starts after a backoff; after a few crashes in a
    /// row the banner's Restart button is left to the user.
    pub(super) fn bridge_exited(&mut self) {
        if let Some(b) = self.bridge.take() {
            match &self.engine {
                Some(engine) => engine.stop(b),
                None => drop(b),
            }
        }
        self.airplay_bridge_changed();
        let ses = &mut self.session;
        ses.bridge_up = false;
        ses.login_window = false;
        ses.signing_out = false;
        self.syncing = false;
        if !self.playback.local {
            self.starting = None;
            self.playback.seek_target = None;
            self.playback.seek_preview = None;
            if self.playback.song.is_some() {
                self.playback.position = self.playback.position_now();
                self.playback.position_at = Some(crate::clock::now());
                self.playback.slew = 0.0;
                self.playback.set_state(PlayState::Paused);
                self.resume = Some(super::resume::Pending::default());
            } else {
                self.playback.set_state(PlayState::Stopped);
            }
            if let Some(t) = &mut self.tray {
                t.set_playing(false);
            }
            self.discord_sync();
        }
        let now = Instant::now();
        let local_only = self.local_only();
        let r = &mut self.revive;
        r.crashes.retain(|t| now.duration_since(*t) < CRASH_WINDOW);
        r.crashes.push(now);
        r.generation += 1;
        let generation = r.generation;
        let log = crate::bridge::log_path().display().to_string();
        match REVIVE_AFTER.get(r.crashes.len() - 1) {
            Some(&wait) if !local_only => {
                r.waiting = true;
                self.session.error = Some("The playback engine stopped. Restarting it\u{2026}".into());
                self.revive_after(wait, generation);
            }
            _ => {
                self.revive.waiting = false;
                self.session.error = Some(format!("The playback engine (Electron) stopped. See {log}."));
            }
        }
    }

    /// Starts a new engine after `wait`, unless something else started or stopped it meanwhile.
    fn revive_after(&self, wait: Duration, generation: u64) {
        let Some(handle) = self.handle.clone() else { return };
        let _ = std::thread::Builder::new().name("bridge-revive".into()).spawn(move || {
            std::thread::sleep(wait);
            handle.update_if(move |s: &mut State, _tx| {
                if s.revive.generation != generation || !s.revive.waiting {
                    return false;
                }
                s.revive.waiting = false;
                s.start_bridge();
                true
            });
        });
    }

    /// The banner's and Settings' Restart: a new engine now.
    pub fn restart_engine(&mut self) {
        self.revive.generation += 1;
        self.revive.waiting = false;
        self.restart_bridge();
    }

    /// The engine is down and waits for the user (no restart pending): the banner offers Restart.
    pub fn engine_needs_restart(&self) -> bool {
        self.bridge.is_none() && !self.revive.waiting && !self.local_only() && self.engine.is_some() && self.session.error.is_some()
    }

    /// Local files only: no bridge, no Apple pages.
    pub fn local_only(&self) -> bool {
        self.settings.music_source == MusicSource::LocalOnly
    }

    /// Starts the bridge if Apple Music is wanted and none runs. Headless runs have no engine.
    pub fn start_bridge(&mut self) {
        self.start_bridge_with(false);
    }

    /// `show_login`: with the sign-in window showing (`--login`). Returns whether it started.
    pub fn start_bridge_with(&mut self, show_login: bool) -> bool {
        if self.bridge.is_some() || self.local_only() {
            return false;
        }
        let Some(engine) = &self.engine else { return false };
        match engine.start(show_login) {
            Ok(b) => {
                self.bridge = Some(b);
                self.airplay_bridge_changed();
                self.session.error = None;
                self.session.bridge_up = false;
                self.session.login_window = show_login;
                true
            }
            Err(e) => {
                eprintln!("uzumaki: could not start the playback bridge: {e}");
                self.session.error = Some(format!("Could not start the playback bridge: {e}. See {}.", engine.log_path().display()));
                false
            }
        }
    }

    /// Stops the bridge (local only mode). An Apple song stops with it and Apple songs leave the
    /// queue; a local file plays on. The sign-in and caches stay.
    pub fn stop_bridge(&mut self) {
        self.drop_apple_playback();
        if let Some(b) = self.bridge.take() {
            match &self.engine {
                Some(engine) => engine.stop(b),
                None => drop(b),
            }
        }
        self.airplay_bridge_changed();
        let ses = &mut self.session;
        ses.bridge_up = false;
        ses.login_window = false;
        ses.login_after_ready = false;
        ses.signing_out = false;
        ses.error = None;
        self.syncing = false;
    }

    /// Apple songs can't play without the bridge: they leave the player and the queue.
    fn drop_apple_playback(&mut self) {
        let apple_now = self.playback.song.as_ref().is_some_and(|s| !s.is_local());
        if apple_now {
            self.queue = PlayQueue::default();
            self.playback.song = None;
            self.playback.set_state(PlayState::Stopped);
            self.discord_sync();
            self.playback.position = 0.0;
            self.playback.position_at = None;
            self.starting = None;
            if let Some(t) = &mut self.tray {
                t.set_song(None);
                t.set_playing(false);
            }
        } else {
            self.queue.keep_local();
        }
    }

    /// Settings → Account → Music source, and the onboarding's choice. Local only stops the
    /// bridge and leaves the Apple pages; Apple Music starts it and opens the sign-in window
    /// once it is up, unless signed in. Returns whether a bridge was started.
    pub fn set_music_source(&mut self, source: MusicSource) -> bool {
        if self.settings.music_source != source {
            self.settings.music_source = source;
            self.save_settings();
        }
        match source {
            MusicSource::LocalOnly => {
                self.stop_bridge();
                self.ui.search_tab = crate::ui::SearchTab::Local;
                self.back_stack.retain(|r| !r.needs_apple());
                if self.route.needs_apple() {
                    self.route = Route::LocalFiles;
                    self.ui.route_changed();
                    self.remember_route();
                }
            }
            MusicSource::Apple => {
                // Albums, Artists and Folders of the music folders are local only mode's.
                self.back_stack.retain(|r| !r.local_library());
                if self.route.local_library() {
                    self.route = Route::LocalFiles;
                    self.ui.route_changed();
                    self.remember_route();
                }
                return self.start_bridge_with(false);
            }
        }
        false
    }

    /// Settings → Account → Apple Music: starts the bridge and, unless signed in, opens the
    /// guided sign-in step on its own (`ui::onboarding::sign_in_only`).
    pub fn switch_to_apple(&mut self) {
        let was_local = self.local_only();
        let started = self.set_music_source(MusicSource::Apple);
        if (was_local || started) && !self.session.authorized {
            crate::ui::onboarding::sign_in_only(self);
        }
    }

    /// The bridge for a `uzu` command that needs Apple Music, or why there is none.
    pub fn apple_bridge(&self) -> Result<Bridge<Msg>, String> {
        if self.local_only() {
            return Err(local_only_answer("that"));
        }
        self.bridge.clone().ok_or_else(|| "the playback bridge isn't running".to_string())
    }
}
