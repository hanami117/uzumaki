//! Discord Rich Presence (`crate::discord`): what `State` tells it, and the Settings → General
//! switches. The snapshot is built after every playback message, but only while the worker runs
//! (the setting on): with it off, none of this costs anything.

use super::*;
use crate::bridge::PlayState;
use crate::discord::Snapshot;

impl State {
    /// Tells Discord what is playing now. Cheap; nothing is sent unless the card would change.
    pub(super) fn discord_sync(&mut self) {
        if self.discord.active() {
            let snap = self.discord_snapshot();
            self.discord.update(snap.as_ref());
        }
    }

    /// The song playing (or paused), None when nothing is. A song that was just asked for counts
    /// as playing: MusicKit reports the old queue stopping first.
    fn discord_snapshot(&self) -> Option<Snapshot> {
        let p = &self.playback;
        let song = p.song.as_ref()?;
        let playing = match p.state {
            s if s.is_active() => true,
            PlayState::Paused => false,
            _ if self.starting.is_some() => true,
            _ => return None,
        };
        let prefs = &self.settings.discord;
        let song_url = (prefs.buttons && !prefs.hide_title && !song.is_local())
            .then(|| match crate::share::song_link(song, self.session.storefront.as_deref()) {
                crate::share::Link::Url(u) => Some(u),
                _ => None,
            })
            .flatten();
        Some(Snapshot {
            title: song.title.clone(),
            artist: song.artist.clone(),
            album: song.album.clone(),
            album_is_fallback: song.album_is_fallback,
            artwork_url: if prefs.artwork { song.artwork.as_ref().map(|a| a.url(512)) } else { None },
            song_url,
            playing,
            position: p.position_now(),
            duration: p.duration,
        })
    }

    /// Starts or stops the presence as the settings say (at launch, and after any change).
    pub(super) fn discord_configure(&mut self) {
        self.discord.configure(&self.settings.discord);
        self.discord_sync();
    }

    fn set_discord(&mut self, change: impl FnOnce(&mut crate::discord::Prefs)) {
        let before = self.settings.discord.clone();
        change(&mut self.settings.discord);
        if self.settings.discord != before {
            self.save_settings();
            self.discord_configure();
        }
    }

    /// Settings → General → Show on Discord.
    pub fn set_discord_enabled(&mut self, on: bool) {
        self.set_discord(|d| d.enabled = on);
    }

    pub fn set_discord_artwork(&mut self, on: bool) {
        self.set_discord(|d| d.artwork = on);
    }

    pub fn set_discord_buttons(&mut self, on: bool) {
        self.set_discord(|d| d.buttons = on);
    }

    pub fn set_discord_hide_paused(&mut self, on: bool) {
        self.set_discord(|d| d.hide_when_paused = on);
    }

    pub fn set_discord_status(&mut self, status: crate::discord::StatusDisplay) {
        self.set_discord(|d| d.status = status);
    }

    pub fn set_discord_hide_title(&mut self, on: bool) {
        self.set_discord(|d| d.hide_title = on);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{Artwork, Song};

    fn song(id: &str) -> Song {
        Song {
            id: id.into(),
            catalog_id: None,
            title: "Redbone".into(),
            artist: "Childish Gambino".into(),
            album: "Awaken, My Love!".into(),
            album_is_fallback: false,
            duration: 327.0,
            track_number: None,
            disc_number: None,
            explicit: false,
            artwork: Some(Artwork { template: "https://is1-ssl.mzstatic.com/image/thumb/x/{w}x{h}bb.jpg".into(), bg: None }),
            playable: true,
            genre: None,
            year: None,
            album_id: Some("1161018478".into()),
            artists: Vec::new(),
        }
    }

    fn playing(id: &str) -> State {
        let mut s = State::offline();
        s.playback.song = Some(song(id));
        s.playback.state = PlayState::Playing;
        s.playback.position = 42.0;
        s.playback.duration = 327.0;
        s.session.storefront = Some("it".into());
        s
    }

    #[test]
    fn the_snapshot_is_the_song_its_cover_and_its_apple_music_page() {
        let snap = playing("1161018480").discord_snapshot().unwrap();
        assert_eq!((snap.title.as_str(), snap.artist.as_str(), snap.playing), ("Redbone", "Childish Gambino", true));
        assert_eq!(snap.artwork_url.as_deref(), Some("https://is1-ssl.mzstatic.com/image/thumb/x/512x512bb.jpg"));
        assert_eq!(snap.song_url.as_deref(), Some("https://music.apple.com/it/album/awaken-my-love/1161018478?i=1161018480"));
        assert_eq!((snap.position, snap.duration), (42.0, 327.0));
    }

    #[test]
    fn a_local_file_has_no_page_and_the_choices_leave_things_out() {
        let local = playing(&Song::local_id(std::path::Path::new("/m/a.flac"))).discord_snapshot().unwrap();
        assert_eq!(local.song_url, None, "no Apple Music page for a file");
        let mut s = playing("1161018480");
        s.settings.discord.artwork = false;
        assert_eq!(s.discord_snapshot().unwrap().artwork_url, None, "no cover URL leaves the computer");
        s.settings.discord.buttons = false;
        assert_eq!(s.discord_snapshot().unwrap().song_url, None);
    }

    #[test]
    fn paused_is_a_snapshot_stopped_and_nothing_are_not() {
        let mut s = playing("1");
        s.playback.state = PlayState::Paused;
        assert!(!s.discord_snapshot().unwrap().playing);
        for st in [PlayState::Stopped, PlayState::Ended, PlayState::None, PlayState::Completed] {
            s.playback.state = st;
            assert!(s.discord_snapshot().is_none(), "{st:?}");
        }
        // Just asked for a song: the old queue stopping is not the end of the music.
        s.starting = Some(crate::clock::now());
        s.playback.state = PlayState::Stopped;
        assert!(s.discord_snapshot().unwrap().playing);
        s.playback.song = None;
        assert!(s.discord_snapshot().is_none());
    }

    #[test]
    fn a_state_without_the_app_never_starts_discord() {
        let mut s = playing("1");
        s.set_discord_enabled(true);
        assert!(s.settings.discord.enabled, "the choice is kept");
        assert!(!s.discord.active(), "but headless and tests start no thread and touch no socket");
        s.discord_sync();
    }
}
