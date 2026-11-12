//! Where the music comes from: Apple Music (through the bridge) or local files only
//! (docs/ONBOARDING.md, stage 2). The bridge starts when Apple Music is wanted and stops for
//! local only; the Apple sign-in, caches and settings stay either way (decision 4).

use super::*;
use crate::settings::MusicSource;

impl Route {
    /// Pages that show Apple Music: hidden in local only mode.
    pub fn needs_apple(&self) -> bool {
        match self {
            Route::Search | Route::LocalFiles => false,
            Route::Collection { id, .. } => !crate::local::view::is_album_id(id),
            Route::Artist { id, .. } => !crate::local::view::is_artist_id(id),
            r => !r.local_library(),
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

impl State {
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
