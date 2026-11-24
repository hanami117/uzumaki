//! Local files: music folders on disk, played in-process (no bridge, no mpd).
//!
//! - `scan`: walks the music folders, reads tags with lofty, keeps an index on disk
//!   (`$XDG_CACHE_HOME/uzumaki/local_index.json`) and cover thumbnails (`…/uzumaki/local_art/`).
//! - `player`: a thread that decodes with symphonia (through rodio) and plays through cpal.
//!   It speaks the same `bridge::Event`s as the bridge, so state, MPRIS and the tray treat both
//!   sources alike.
//! - `favourites`: the star on local songs, kept on this computer only.
//! - `lyrics`: a file's own lyrics (`.lrc`/`.txt` beside it, or its tags).
//! - `m3u`: `.m3u8` export and import of playlists.
//! - `tap`: AirPlay's copy of the player's output, made only while a speaker takes the audio.
//!
//! Songs from here are ordinary `models::Song`s whose id is `file://` + path (`Song::is_local`).

pub mod favourites;
pub mod lyrics;
pub mod m3u;
pub mod player;
pub mod scan;
pub mod tap;
pub mod view;

pub use player::LocalPlayer;
pub use scan::LocalTrack;
pub use view::LocalView;

use std::path::PathBuf;

/// `$XDG_CACHE_HOME/uzumaki` (else `~/.cache/uzumaki`).
pub fn cache_base() -> PathBuf {
    #[cfg(windows)]
    return uzumaki_ctl::paths::dir(uzumaki_ctl::paths::Kind::Cache);
    #[cfg(not(windows))]
    std::env::var_os("XDG_CACHE_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .unwrap_or_else(std::env::temp_dir)
        .join("uzumaki")
}
