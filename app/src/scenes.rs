//! Named states for headless renders (`--scene NAME`), filled with fake, deterministic data.

use crate::bridge::PlayState;
use crate::lyrics::{Line, Lyrics};
use crate::models::{Artist, Artwork, Collection, CollectionKind, SearchResults, Shelf, Song, Station};
use crate::settings::{Pin, PinKind};
use crate::state::PlayQueue;
use crate::ui::{Col, Sort};
use crate::state::{LibraryTab, Load, Paged, Route, SHELF_TITLES, State};

pub const SCENES: &[(&str, fn(&mut State))] = &[
    ("home", home),
    ("library-albums", library_albums),
    ("library-songs", library_songs),
    ("search", search),
    ("album", album),
    ("album-motion", album_motion),
    ("album-motion-paused", album_motion_paused),
    ("album-long", |s| { album(s); long_album(s) }),
    ("album-motion-long", |s| { album_motion(s); long_album(s) }),
    ("album-artists-two", album_two_artists),
    ("album-artist-none", album_no_artist),
    ("album-file", album_file),
    ("playlist", playlist),
    ("playlist-songs", playlist_songs),
    ("table-wide", table_scene),
    ("table-medium", table_scene),
    ("table-narrow", table_scene),
    ("table-panel", table_panel),
    ("now-playing", now_playing),
    ("now-playing-nolyrics", now_playing_nolyrics),
    ("now-playing-words", crate::ui::scene_np_words),
    ("now-playing-lines-only", crate::ui::scene_np_lines_only),
    ("now-playing-interlude", crate::ui::scene_np_interlude),
    ("now-playing-intro", crate::ui::scene_np_intro),
    ("now-playing-unsynced", crate::ui::scene_np_unsynced),
    ("now-playing-cover-only", crate::ui::scene_np_cover_only),
    ("now-playing-motion", |s| crate::ui::scene_np_motion(s, false)),
    ("now-playing-motion-lyrics", |s| crate::ui::scene_np_motion(s, true)),
    ("now-playing-queue", now_playing_queue),
    ("now-playing-queue-empty", now_playing_queue_empty),
    ("up-next-stars", up_next_stars),
    ("now-playing-queue-stars", now_playing_queue_stars),
    ("now-playing-no-art", now_playing_no_art),
    ("now-playing-no-art-lyrics", now_playing_no_art_lyrics),
    ("player-bar-no-art", player_bar_no_art),
    ("now-playing-untagged", now_playing_untagged),
    ("player-bar-untagged", player_bar_untagged),
    ("up-next", up_next),
    ("up-next-station", up_next_station),
    ("now-playing-queue-station", now_playing_queue_station),
    ("up-next-long-history", up_next_long_history),
    ("up-next-no-history", up_next_no_history),
    ("now-playing-queue-long-history", now_playing_queue_long_history),
    ("now-playing-queue-no-history", now_playing_queue_no_history),
    ("up-next-repeat", up_next_repeat),
    ("up-next-repeat-end", up_next_repeat_end),
    ("up-next-autoplay", up_next_autoplay),
    ("up-next-autoplay-playing", up_next_autoplay_playing),
    ("up-next-autoplay-off", up_next_autoplay_off),
    ("now-playing-queue-autoplay", now_playing_queue_autoplay),
    ("menu-song-station", menu_song_station),
    ("menu-artist-station", menu_artist_station),
    ("right-panel-lyrics", right_panel_lyrics),
    ("right-panel-queue", up_next),
    ("bg-right-panel", bg_right_panel),
    ("bg-wallpaper", bg_wallpaper),
    ("right-panel-narrow", right_panel_lyrics),
    ("new-playlist", new_playlist),
    ("new-playlist-picture", new_playlist_picture),
    ("edit-playlist", edit_playlist),
    ("edit-playlist-picture-only", edit_playlist_picture_only),
    ("selection", selection),
    ("context-menu", context_menu),
    ("new", new),
    ("radio", radio),
    ("made-for-you", made_for_you),
    ("recently-added", recently_added),
    ("artists", artists),
    ("artist", artist),
    ("artist-full", artist_full),
    ("artist-full-portrait", artist_full_portrait),
    ("artist-full-bg", artist_full_bg),
    ("artist-see-all", artist_see_all),
    ("artist-library-fallback", artist_library_fallback),
    ("search-library", search_library),
    ("search-apple", search_apple),
    ("search-bg", search_bg),
    ("sidebar-pins", sidebar_pins),
    ("sidebar-playlist-art", sidebar_playlist_art),
    ("sidebar-collapsed", sidebar_collapsed),
    ("sidebar-collapsed-playlists", sidebar_collapsed_playlists),
    ("drag-songs-rail", drag_songs_rail),
    ("drag-pins", drag_pins),
    ("drag-songs", drag_songs),
    ("narrow", narrow),
    ("compact", compact),
    ("menu-pin", menu_pin),
    ("menu-pin-playlist", menu_pin_playlist),
    ("menu-pin-rail", menu_pin_rail),
    ("menu-card", menu_card),
    ("share-album-card", share_album_card),
    ("share-album-header", share_album_header),
    ("share-playlist", share_playlist),
    ("share-playlist-library-hidden", share_playlist_library_hidden),
    ("share-artist", share_artist),
    ("share-station", share_station),
    ("share-song-player-bar", share_song_player_bar),
    ("share-song-row", share_song_row),
    ("share-library-album", share_library_album),
    ("menu-submenu", menu_submenu),
    ("signed-out", signed_out),
    ("starting", starting),
    ("error", error),
    ("settings-account", settings_account),
    ("settings-search-empty", settings_search_empty),
    ("settings-search", settings_search),
    ("settings-search-none", settings_search_none),
    ("settings-search-flash", settings_search_flash),
    ("settings-signed-out", settings_signed_out),
    ("settings-sign-out-confirm", settings_sign_out_confirm),
    ("settings-playback", settings_playback),
    ("settings-general", settings_general),
    ("settings-about", settings_about),
    ("settings-playback-local", settings_playback_local),
    ("settings-check-now", settings_check_now),
    ("settings-check-now-running", settings_check_now_running),
    ("update-check-toast", update_check_toast),
    ("update-check-failed", update_check_failed),
    ("resume-restored", resume_restored),
    ("update-notice", update_notice),
    ("update-notice-package", update_notice_package),
    ("update-notice-homebrew", update_notice_homebrew),
    ("update-notice-aur", update_notice_aur),
    ("update-required", update_required),
    ("update-required-aur", update_required_aur),
    ("update-notice-narrow", update_notice),
    ("settings-keyboard-edit", settings_keyboard_edit),
    ("menu-now-playing", menu_now_playing),
    ("sidebar-hidden", sidebar_hidden),
    ("appearance-preview", appearance_preview),
    ("settings-keyboard", settings_keyboard),
    ("local-files", local_files),
    ("local-files-empty", local_files_empty),
    ("local-files-scanning", local_files_scanning),
    ("settings-local", settings_local),
    ("search-local", search_local),
    ("appearance", appearance),
    ("appearance-icon-auto", appearance_icon_auto),
    ("intro", intro),
    ("onboarding", onboarding),
    ("onboarding-welcome", |s| onboarding_at(s, Mode::FirstRun, Step::Welcome, None)),
    ("onboarding-choice", |s| onboarding_at(s, Mode::FirstRun, Step::Choice, None)),
    ("onboarding-choice-picked", |s| onboarding_at(s, Mode::FirstRun, Step::Choice, Some(MusicSource::LocalOnly))),
    ("onboarding-done", |s| onboarding_at(s, Mode::FirstRun, Step::Done, Some(MusicSource::Apple))),
    ("onboarding-done-local", |s| onboarding_at(s, Mode::FirstRun, Step::Done, Some(MusicSource::LocalOnly))),
    ("onboarding-replay", onboarding_replay),
    ("onboarding-replay-done", |s| onboarding_at(s, Mode::Replay, Step::Done, None)),
    ("settings-show-tour", settings_general),
    ("onboarding-signin-starting", |s| onboarding_sign_in(s, 0)),
    ("onboarding-signin-lead", |s| onboarding_sign_in(s, 1)),
    ("onboarding-signin-waiting", |s| onboarding_sign_in(s, 2)),
    ("onboarding-signin-closed", |s| onboarding_sign_in(s, 3)),
    ("onboarding-signin-done", |s| onboarding_sign_in(s, 4)),
    ("onboarding-signin-done-nostore", |s| onboarding_sign_in(s, 5)),
    ("onboarding-signin-error", |s| onboarding_sign_in(s, 6)),
    ("onboarding-signin-only", onboarding_sign_in_only),
    ("onboarding-done-synced", onboarding_done_syncing),
    ("onboarding-tour-play", |s| onboarding_tour(s, false, 0, &[])),
    ("onboarding-tour-play-tried", |s| onboarding_tour(s, false, 0, &["play_pause"])),
    ("onboarding-tour-sidebar", |s| onboarding_tour(s, false, 1, &[])),
    ("onboarding-tour-sidebar-tried", |s| onboarding_tour(s, false, 1, &["toggle_sidebar"])),
    ("onboarding-tour-right", |s| onboarding_tour(s, false, 2, &[])),
    ("onboarding-tour-right-tried", |s| onboarding_tour(s, false, 2, &["toggle_right_panel"])),
    ("onboarding-tour-right-settled", |s| onboarding_tour(s, false, 2, &["toggle_right_panel", "settle"])),
    ("onboarding-tour-right-off", |s| onboarding_tour(s, false, 2, &["toggle_right_panel", "toggle_right_panel", "settle"])),
    ("onboarding-tour-panels-switched", |s| onboarding_tour(s, false, 3, &["show_queue", "show_lyrics", "settle"])),
    ("onboarding-tour-panels-off", |s| onboarding_tour(s, false, 3, &["show_queue", "show_queue", "settle"])),
    ("onboarding-tour-sidebar-off", |s| onboarding_tour(s, false, 1, &["toggle_sidebar", "toggle_sidebar", "settle"])),
    ("onboarding-tour-skip-settled", |s| onboarding_tour(s, false, 6, &["next", "next", "settle"])),
    ("onboarding-tour-more-settled", |s| onboarding_tour(s, false, 7, &["slot3", "settle"])),
    ("onboarding-tour-panels", |s| onboarding_tour(s, false, 3, &[])),
    ("onboarding-tour-panels-tried", |s| onboarding_tour(s, false, 3, &["show_lyrics"])),
    ("onboarding-tour-now-playing", |s| onboarding_tour(s, false, 4, &[])),
    ("onboarding-tour-now-playing-tried", |s| onboarding_tour(s, false, 4, &["now_playing"])),
    ("onboarding-tour-find", |s| onboarding_tour(s, false, 5, &[])),
    ("onboarding-tour-find-tried", |s| onboarding_tour(s, false, 5, &["find"])),
    ("onboarding-tour-skip", |s| onboarding_tour(s, false, 6, &[])),
    ("onboarding-tour-skip-tried", |s| onboarding_tour(s, false, 6, &["next"])),
    ("onboarding-tour-more", |s| onboarding_tour(s, false, 7, &[])),
    ("onboarding-tour-more-tried", |s| onboarding_tour(s, false, 7, &["slot3"])),
    ("onboarding-tour-local-play", |s| onboarding_tour(s, true, 0, &[])),
    ("onboarding-tour-local-play-tried", |s| onboarding_tour(s, true, 0, &["play_pause"])),
    ("onboarding-tour-local-sidebar", |s| onboarding_tour(s, true, 1, &[])),
    ("onboarding-tour-local-sidebar-tried", |s| onboarding_tour(s, true, 1, &["toggle_sidebar"])),
    ("onboarding-tour-local-right", |s| onboarding_tour(s, true, 2, &[])),
    ("onboarding-tour-local-right-tried", |s| onboarding_tour(s, true, 2, &["toggle_right_panel"])),
    ("onboarding-tour-local-panels", |s| onboarding_tour(s, true, 3, &[])),
    ("onboarding-tour-local-panels-tried", |s| onboarding_tour(s, true, 3, &["show_queue"])),
    ("onboarding-tour-local-now-playing", |s| onboarding_tour(s, true, 4, &[])),
    ("onboarding-tour-local-now-playing-tried", |s| onboarding_tour(s, true, 4, &["now_playing"])),
    ("onboarding-tour-local-find", |s| onboarding_tour(s, true, 5, &[])),
    ("onboarding-tour-local-find-tried", |s| onboarding_tour(s, true, 5, &["find"])),
    ("onboarding-tour-local-skip", |s| onboarding_tour(s, true, 6, &[])),
    ("onboarding-tour-local-skip-tried", |s| onboarding_tour(s, true, 6, &["next"])),
    ("onboarding-tour-local-more", |s| onboarding_tour(s, true, 7, &[])),
    ("onboarding-tour-local-more-tried", |s| onboarding_tour(s, true, 7, &["slot2"])),
    ("onboarding-tour-more-settings", |s| onboarding_tour(s, false, 7, &["settings"])),
    ("onboarding-tour-more-shortcuts", |s| onboarding_tour(s, false, 7, &["shortcuts"])),
    ("onboarding-tour-panels-queue", |s| onboarding_tour(s, false, 3, &["show_queue"])),
    ("onboarding-tour-skip-back", |s| onboarding_tour(s, false, 6, &["previous"])),
    ("onboarding-tour-rebound", onboarding_tour_rebound),
    ("onboarding-tour-more-pins", onboarding_tour_pins),
    ("onboarding-replay-tour", onboarding_replay_tour),
    ("onboarding-folders-empty", |s| onboarding_folders(s, 0, false)),
    ("onboarding-folders", |s| onboarding_folders(s, 2, false)),
    ("onboarding-folders-scanning", |s| onboarding_folders(s, 1, true)),
    ("onboarding-folders-unticked", |s| onboarding_folders(s, 3, false)),
    ("local-only-sidebar", local_only_sidebar),
    ("local-only-rail", local_only_rail),
    ("local-only-empty", local_only_empty),
    ("local-only-search", local_only_search),
    ("local-only-songs", local_only_songs),
    ("local-only-albums", local_only_albums),
    ("local-only-artists", local_only_artists),
    ("local-only-artist", local_only_artist),
    ("local-only-album", local_only_album),
    ("local-only-album-untagged", local_only_album_untagged),
    ("local-only-folders", local_only_folders),
    ("local-only-folder", local_only_folder),
    ("local-only-folder-untagged", local_only_folder_untagged),
    ("local-only-20k-songs", local_only_big_songs),
    ("local-only-20k-albums", local_only_big_albums),
    ("local-only-20k-artists", local_only_big_artists),
    ("local-only-20k-artist", local_only_big_artist),
    ("local-only-20k-folder", local_only_big_folder),
    ("settings-music-source", settings_music_source),
    ("settings-music-source-local", settings_music_source_local),
    ("intro-sync", intro_sync),
    ("intro-starting", intro_starting),
    ("sync-pill", sync_pill),
    ("sync-pill-floating", sync_pill_floating),
    ("sync-pill-done", sync_pill_done),
    ("appearance-picture", appearance_picture),
    ("bg-list-cards-only", bg_list_cards_only),
    ("bg-list-faint", bg_list_faint),
    ("bg-dither", bg_dither),
    ("bg-ascii", bg_ascii),
    ("bg-halftone", bg_halftone),
    ("bg-blur", bg_blur),
    ("bg-picture-plain", |s| bg_picture(s, crate::appearance::Effect::Plain)),
    ("bg-picture-dither", |s| bg_picture(s, crate::appearance::Effect::Dither)),
    ("bg-picture-ascii", |s| bg_picture(s, crate::appearance::Effect::Ascii)),
    ("bg-picture-halftone", |s| bg_picture(s, crate::appearance::Effect::Halftone)),
    ("bg-picture-scanlines", |s| bg_picture(s, crate::appearance::Effect::Scanlines)),
    ("bg-picture-blur", |s| bg_picture(s, crate::appearance::Effect::Blur)),
    ("bg-picture-album", bg_picture_album),
    ("bg-picture-songs", bg_picture_songs),
    ("bg-strong", bg_strong),
    ("np-bg-cover", np_bg_cover),
    ("np-bg-cover-halftone", |s| np_bg_cover_with(s, crate::appearance::Effect::Halftone)),
    ("np-bg-cover-dither", |s| np_bg_cover_with(s, crate::appearance::Effect::Dither)),
    ("np-bg-cover-ascii", |s| np_bg_cover_with(s, crate::appearance::Effect::Ascii)),
    ("np-bg-app", np_bg_app),
    ("np-slide", np_slide),
    ("player-floating", player_floating),
    ("player-floating-narrow", player_floating),
    ("volume-muted", volume_muted),
    ("volume-low", volume_low),
    ("volume-mid", volume_mid),
    ("fav-table", fav_table),
    ("fav-album", fav_album),
    ("fav-menu", fav_menu),
    ("fav-card-menu", fav_card_menu),
    ("fav-np", fav_np),
    ("bar-long-title", bar_long_title),
    ("bar-long-title-floating", bar_long_title_floating),
];

// (title, artist, year, songs)
const ALBUMS: [(&str, &str, &str, u32); 8] = [
    ("Low Tide Radio", "Mara Quell", "2025", 9),
    ("Concrete Bloom", "Okafor & the Lines", "2024", 9),
    ("Night Bus Hymns", "Juniper Vale", "2026", 9),
    ("Static Garden", "Telu", "2023", 5),
    ("Paper Satellites", "Ines Rook", "2025", 9),
    ("Slow Weather", "The Margins", "2022", 9),
    ("Glass Hours", "Kenji Arlo", "2026", 2),
    ("Northern Lots", "Sable Fen", "2024", 9),
];

const WORDS: [&str; 15] = [
    "Harbor", "Signal", "Under Neon", "Small Rooms", "Driftwood", "Late Train", "Afterglow", "Copper Sky", "Feral",
    "Quiet Engine", "Halflight", "Overpass", "Saltwater", "Echo Park", "Kites",
];

const GENRES: [Option<&str>; 6] =
    [Some("Alternative"), Some("Electronic"), Some("Hip-Hop/Rap"), None, Some("Singer/Songwriter"), Some("Jazz")];

fn song(album: usize, k: u32) -> Song {
    let (title, artist, _, _) = ALBUMS[album % 8];
    let mut name = WORDS[(album * 7 + k as usize * 4) % 15].to_string();
    if album == 2 && k == 4 {
        name = "A Very Long Track Title That Should Be Truncated With an Ellipsis (Extended Anniversary Mix)".into();
    }
    Song {
        id: format!("s-{album}-{k}"),
        catalog_id: None,
        title: name,
        artist: artist.into(),
        album: title.into(),
        album_is_fallback: false,
        duration: (150 + (album as u32 * 37 + k * 53) % 140) as f64,
        track_number: Some(k + 1),
        disc_number: Some(1),
        explicit: false,
        artwork: None,
        playable: true,
        genre: GENRES[(album * 3 + k as usize) % GENRES.len()].map(String::from),
        year: Some((2018 + (album as u32 * 5 + k * 3) % 9).to_string()),
        album_id: None,
        artists: Vec::new(),
    }
}

fn album_item(i: usize, with_tracks: bool) -> Collection {
    let (title, artist, year, n) = ALBUMS[i % 8];
    // Beyond the first 8, vary titles so big grids look different.
    let title = if i >= 8 { format!("{title} {}", i / 8 + 1) } else { title.to_string() };
    let title = if i == 3 { "Static Garden: The Complete Sessions, Remastered Deluxe Edition".to_string() } else { title };
    Collection {
        kind: CollectionKind::Album,
        id: format!("a{i}"),
        title,
        subtitle: artist.into(),
        artwork: None,
        year: Some(year.into()),
        genre: Some("Alternative".into()),
        description: None,
        track_count: Some(n),
        modified: None,
        editable: false,
        favorites: false,
        public: None,
        // Album 5 has no artist id (plain text); album 6 has two (a menu); the rest one.
        artists: match i % 8 {
            5 => vec![],
            6 => vec![
                Artist { id: "ar6".into(), name: artist.into() },
                Artist { id: "ar7".into(), name: "Guest Voice".into() },
            ],
            n => vec![Artist { id: format!("ar{n}"), name: artist.into() }],
        },
        tracks: if with_tracks { (0..n).map(|k| song(i, k)).collect() } else { vec![] },
    }
}

fn playlist_item(i: usize, name: &str) -> Collection {
    Collection {
        kind: CollectionKind::Playlist,
        id: format!("p.{i}"),
        title: name.into(),
        subtitle: "Apple Music".into(),
        artwork: None,
        year: None,
        genre: None,
        description: None,
        track_count: Some(24),
        modified: None,
        editable: false,
        favorites: false,
        public: None,
        artists: vec![],
        tracks: vec![],
    }
}

const PLAYLISTS: [&str; 4] = ["Morning commute", "Focus", "Late night", "Summer 2026"];

fn base(s: &mut State) {
    s.session.bridge_up = true;
    s.session.authorized = true;
    let mut lists: Vec<Collection> = PLAYLISTS.iter().enumerate().map(|(i, n)| playlist_item(i, n)).collect();
    for c in &mut lists {
        c.editable = true;
    }
    let mut fav = playlist_item(90, "Favourite Songs");
    fav.favorites = true;
    lists.insert(2, fav);
    s.library_playlists = Paged::ready(lists);
    let mut now = song(2, 1);
    now.duration = 241.0;
    s.playback.song = Some(now);
    s.playback.state = PlayState::Playing;
    s.playback.position = 83.0;
    s.playback.duration = 241.0;
    s.playback.volume = 0.7;
}

fn home(s: &mut State) {
    base(s);
    let recent: Vec<Collection> = [2, 3, 4, 6, 7, 5].iter().map(|&i| album_item(i, false)).collect();
    let added: Vec<Collection> = (0..8).map(|i| album_item(i, false)).collect();
    s.shelves[0] = Load::Ready(Shelf { title: SHELF_TITLES[0].into(), items: recent, songs: vec![], stations: vec![] });
    s.shelves[1] = Load::Ready(Shelf { title: SHELF_TITLES[1].into(), items: added, songs: vec![], stations: vec![] });
}

fn library_albums(s: &mut State) {
    base(s);
    s.route = Route::Library(LibraryTab::Albums);
    s.library_albums = Paged::ready((0..64).map(|i| album_item(i, false)).collect());
}

fn library_songs(s: &mut State) {
    base(s);
    s.route = Route::Library(LibraryTab::Songs);
    let items = (0..400).map(|i| song(i % 8, (i / 8) as u32 % 9)).collect();
    s.library_songs = Paged::ready(items);
}

fn search(s: &mut State) {
    base(s);
    s.route = Route::Search;
    s.search_query = "night".into();
    s.search_seq = 1;
    s.search = Load::Ready(SearchResults {
        songs: (0..8).map(|k| song(2, k)).collect(),
        albums: [2, 5, 7, 0, 4].iter().map(|&i| album_item(i, false)).collect(),
        playlists: vec![playlist_item(2, "Late night"), playlist_item(5, "Night shift: city lights and long drives")],
    });
}

fn album(s: &mut State) {
    base(s);
    let mut c = album_item(2, true);
    c.description = Some(
        "Recorded over one winter in a converted bus depot, the record follows the last service of the evening \
         from the first stop to the terminus. Quiet synths, brushed drums and a choir of friends. It was mixed \
         live to tape across a single weekend and never touched again."
            .into(),
    );
    s.back_stack.push(Route::Home);
    s.route = Route::Collection { kind: CollectionKind::Album, id: c.id.clone() };
    s.collections.insert(c.id.clone(), Load::Ready(c));
}

/// `album` as a catalog album with animated artwork (the local fixture clip): the cover
/// crossfades to the loop after its first frame. `--frames`/`--at` to see it move; as for
/// `now-playing-motion`, the Mac's player is deterministic headless, the ffmpeg path real time.
fn album_motion(s: &mut State) {
    album(s);
    let Route::Collection { id, .. } = &s.route else { return };
    let Some(Load::Ready(mut c)) = s.collections.remove(id) else { return };
    c.id = "1440857781".into();
    s.route = Route::Collection { kind: CollectionKind::Album, id: c.id.clone() };
    crate::ui::scene_clip(s, &c.id);
    s.collections.insert(c.id.clone(), Load::Ready(c));
}

/// The shown album page with 40 songs, for measuring scrolling (`--script`).
fn long_album(s: &mut State) {
    let Route::Collection { id, .. } = &s.route else { return };
    if let Some(Load::Ready(c)) = s.collections.get_mut(id) {
        let base = c.tracks.clone();
        c.tracks = (0..40).map(|k| {
            let mut t = base[k % base.len()].clone();
            t.id = format!("{}-{k}", t.id);
            t.track_number = Some(k as u32 + 1);
            t
        }).collect();
    }
}

/// `album-motion` with the music paused: the album page's loop plays anyway, as Apple's does.
fn album_motion_paused(s: &mut State) {
    album_motion(s);
    s.playback.state = PlayState::Paused;
}

/// Album 6: two artist ids, so the name opens a menu of them.
fn album_two_artists(s: &mut State) {
    base(s);
    let c = album_item(6, true);
    s.back_stack.push(Route::Home);
    s.route = Route::Collection { kind: CollectionKind::Album, id: c.id.clone() };
    s.collections.insert(c.id.clone(), Load::Ready(c));
}

/// Album 5: no artist id (an old cache file, or Apple sent none), so the name stays plain text.
fn album_no_artist(s: &mut State) {
    base(s);
    let c = album_item(5, true);
    s.back_stack.push(Route::Home);
    s.route = Route::Collection { kind: CollectionKind::Album, id: c.id.clone() };
    s.collections.insert(c.id.clone(), Load::Ready(c));
}

/// A real album or playlist page from the JSON file `UZUMAKI_SCENE_FILE`: Apple's reply
/// (`{"data": [..]}`, or a bridge reply with it under `result`) or a library cache file
/// (`collections/<id>.json`). For checking live data headless; nothing to show without one.
fn album_file(s: &mut State) {
    base(s);
    let Some(text) = std::env::var_os("UZUMAKI_SCENE_FILE").and_then(|p| std::fs::read_to_string(p).ok()) else { return };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else { return };
    let v = if v["result"].is_object() { &v["result"] } else { &v };
    let c = if v["data"].is_object() {
        serde_json::from_value::<Collection>(v["data"].clone()).ok()
    } else {
        Collection::parse(&v["data"][0])
    };
    let Some(c) = c else { return };
    s.back_stack.push(Route::Home);
    s.route = Route::Collection { kind: c.kind, id: c.id.clone() };
    s.collections.insert(c.id.clone(), Load::Ready(c));
}

fn playlist(s: &mut State) {
    playlist_page(s);
}

/// The same playlist shown as the Songs table, sorted by Artist descending.
fn playlist_songs(s: &mut State) {
    playlist_page(s);
    s.ui.table.songs_view = true;
    s.ui.table.pl_sort = Sort { col: Some(Col::Artist), asc: false };
}

/// The Songs table for the column scenes `table-wide` (1500x800), `table-medium` (960x800) and
/// `table-narrow` (480x800): one scene, the window width decides which columns show.
fn table_scene(s: &mut State) {
    playlist_page(s);
    s.ui.table.songs_view = true;
}

/// The table at `--size 1300x800` with the right sidebar open; a script taps `rp-close` to widen it.
fn table_panel(s: &mut State) {
    table_scene(s);
    s.settings.right_panel = true;
}

fn playlist_page(s: &mut State) {
    // The artwork list: scenes built on this page (selection, menus, drag and drop) were drawn
    // for it. The Songs view is the app's default now; `playlist_songs` shows that one.
    s.ui.table.songs_view = false;
    base(s);
    let mut c = playlist_item(0, "Morning commute");
    c.description = Some("Gentle starts for early trains.".into());
    c.tracks = (0..14).map(|i| song(i % 8, (i / 8) as u32 + 1)).enumerate().map(|(i, mut t)| {
        t.id = format!("pl-{i}");
        t
    }).collect();
    // Make the playing song one of the rows.
    s.playback.song = Some(c.tracks[3].clone());
    s.back_stack.push(Route::Home);
    s.route = Route::Collection { kind: CollectionKind::Playlist, id: c.id.clone() };
    s.collections.insert(c.id.clone(), Load::Ready(c));
}

/// Original lyrics for the scenes (a second voice answers on some lines).
fn fake_lyrics() -> Lyrics {
    const LINES: [(&str, bool); 14] = [
        ("The last tram leaves without a sound", false),
        ("and the depot lights go low", false),
        ("I fold the map along its seams", false),
        ("somewhere past the overpass", false),
        ("(we never needed the map)", true),
        ("Every window holds a different sky", false),
        ("copper, then a quiet blue", false),
        ("and the driver hums a tune I almost know", false),
        ("a tune you almost wrote", false),
        ("(hold the door, hold the door)", true),
        ("Terminus is only a word", false),
        ("for the place we stop pretending", false),
        ("so we walk the rest of the way", false),
        ("singing it back to the dark", false),
    ];
    Lyrics {
        synced: true,
        lines: LINES
            .iter()
            .enumerate()
            .map(|(i, (text, second))| Line {
                start: 6.0 * i as f64,
                end: 6.0 * (i as f64 + 1.0),
                text: (*text).into(),
                second_voice: *second,
                words: Vec::new(),
            })
            .collect(),
    }
}

fn now_playing_base(s: &mut State) {
    album(s);
    let mut now = song(2, 1);
    now.catalog_id = Some("cat-now".into());
    now.title = "Last Service".into();
    now.duration = 241.0;
    now.artwork = Some(Artwork { template: "https://example.invalid/{w}x{h}.{f}".into(), bg: Some("a79be6".into()) });
    s.playback.song = Some(now);
    s.playback.position = 45.0; // line 7 is current
    s.playback.shuffle = true;
    s.playback.repeat = crate::state::Repeat::One;
    s.ui.now_playing = true;
}

fn now_playing(s: &mut State) {
    now_playing_base(s);
    s.lyrics.insert("cat-now".into(), Load::Ready(Some(fake_lyrics())));
}

fn now_playing_nolyrics(s: &mut State) {
    now_playing_base(s);
    s.lyrics.insert("cat-now".into(), Load::Ready(None));
}

/// A song with no artwork (a local file without a cover, say): the initials tile, big.
fn now_playing_no_art(s: &mut State) {
    now_playing_nolyrics(s);
    if let Some(now) = s.playback.song.as_mut() {
        now.artwork = None;
    }
}

/// The same beside the lyrics (a smaller cover).
fn now_playing_no_art_lyrics(s: &mut State) {
    now_playing(s);
    if let Some(now) = s.playback.song.as_mut() {
        now.artwork = None;
    }
}

/// An untagged local file: the album is only its folder's name ("Music"), so the tile shows the
/// title's initials ("PN"), not the folder's.
fn now_playing_untagged(s: &mut State) {
    now_playing_no_art(s);
    if let Some(now) = s.playback.song.as_mut() {
        now.id = "file:///home/user/Music/powerade.mp3".into();
        now.title = "Powerade (NOISETIME x Hähnchenteile Remix)".into();
        now.artist = "Unknown Artist".into();
        now.album = "Music".into();
        now.album_is_fallback = true;
    }
}

fn player_bar_untagged(s: &mut State) {
    now_playing_untagged(s);
    s.ui.now_playing = false;
}

/// Now Playing closed: the player bar's cover for a song with no artwork.
fn player_bar_no_art(s: &mut State) {
    now_playing_no_art(s);
    s.ui.now_playing = false;
}

fn up_next(s: &mut State) {
    album(s);
    let context: Vec<Song> = (0..40).map(|i| song(i % 8, (i / 8) as u32 + 2)).enumerate().map(|(i, mut t)| {
        t.id = format!("ctx-{i}");
        t
    }).collect();
    let user: std::collections::VecDeque<Song> = (0..3).map(|k| {
        let mut t = song(5, k);
        t.id = format!("user-{k}");
        t
    }).collect();
    for song in context.iter().take(3) { s.history.heard(song); }
    s.queue = PlayQueue::start(&context, 2, false, "Night Bus Hymns", user);
    s.playback.song = s.queue.current().cloned();
    s.settings.right_panel = true;
    s.settings.right_tab = crate::settings::RightTab::Queue;
}

/// Repeat all with the current lap nearly done (two songs left): the divider and the next lap.
fn up_next_repeat(s: &mut State) {
    up_next(s);
    s.playback.repeat = crate::state::Repeat::All;
    s.queue.user.clear();
    let n = s.queue.context.len();
    s.queue.pos = n - 3;
    s.queue.plan_lap(crate::state::Repeat::All);
}

/// The same on the lap's last song, with shuffle on: nothing left of this lap, only the planned next one.
fn up_next_repeat_end(s: &mut State) {
    up_next_repeat(s);
    s.queue.pos = s.queue.context.len() - 1;
    s.queue.plan_lap(crate::state::Repeat::All);
}

/// AutoPlay on, near the end of the list (two songs left, one queued by hand): the "AutoPlay"
/// header and Apple's suggestions follow the list, dimmed; ∞ is lit on the Now Playing label.
fn up_next_autoplay(s: &mut State) {
    up_next(s);
    s.session.authorized = true;
    s.queue.user.truncate(1);
    let n = s.queue.context.len();
    s.queue.pos = n - 3;
    s.playback.song = s.queue.current().cloned();
    let suggested: Vec<Song> = (0..8).map(|i| {
        let mut t = song((i + 3) % 8, 20 + i as u32);
        t.id = format!("auto-{i}");
        t
    }).collect();
    s.queue.add_autoplay(suggested);
}

/// The list has played out: an AutoPlay song plays, more suggestions follow it.
fn up_next_autoplay_playing(s: &mut State) {
    up_next_autoplay(s);
    s.queue.user.clear();
    let mut q = s.queue.clone();
    q.pos = q.context.len() - 1;
    s.queue = q;
    // MusicKit reports the first suggestion: it joins the list and plays.
    let first = s.queue.autoplay[0].clone();
    s.queue.advance_to(&first);
    s.playback.song = Some(first);
}

/// AutoPlay switched off with ∞: no suggestions, the switch unlit.
fn up_next_autoplay_off(s: &mut State) {
    up_next_autoplay(s);
    s.settings.autoplay = false;
    s.queue.autoplay.clear();
}

fn now_playing_queue_autoplay(s: &mut State) {
    up_next_autoplay(s);
    s.settings.right_panel = false;
    s.settings.now_playing_queue = true;
    s.ui.now_playing = true;
}

/// Right-click on one song of a playlist: "Create Station" ends the playing group.
fn menu_song_station(s: &mut State) {
    playlist_page(s);
    let song = song(1, 4);
    crate::ui::scene_song_menu_list(s, song, [420.0, 330.0]);
}

/// A catalog artist page's ••• menu: Create Station, then Pin.
fn menu_artist_station(s: &mut State) {
    artist_full(s);
    let a = crate::models::Artist { id: "28673423".into(), name: "Mara Quell".into() };
    crate::ui::scene_menu_artist(s, &a, [560.0, 330.0]);
}

/// A radio station playing, one song queued by hand, and five songs the station already played.
fn up_next_station(s: &mut State) {
    up_next(s);
    let songs: Vec<Song> = s.queue.context.clone();
    let mut q = PlayQueue::for_station("ra.20", "Stazione di Thomas", None);
    for song in songs.iter().take(6).rev() {
        q.station_heard(song);
    }
    q.add_to_queue(&[songs[7].clone()]);
    s.history = Default::default();
    for song in songs.iter().take(6).rev() { s.history.heard(song); }
    s.queue = q;
    s.playback.song = songs.first().cloned();
}

/// The same in full-screen Now Playing's queue column.
fn now_playing_queue_station(s: &mut State) {
    up_next_station(s);
    s.settings.right_panel = false;
    s.settings.now_playing_queue = true;
    s.ui.now_playing = true;
}

/// Full-screen Now Playing with Up Next in the lyrics' place (the queue button).
fn now_playing_queue(s: &mut State) {
    up_next(s);
    s.settings.right_panel = false;
    s.settings.now_playing_queue = true;
    s.ui.now_playing = true;
}

/// Up Next with ten songs of session History above Now Playing (the progressive dimming).
fn up_next_long_history(s: &mut State) {
    up_next(s);
    s.history = Default::default();
    for i in 0..10 {
        let mut t = song(i % 8, 9 + i as u32);
        t.id = format!("hist-{i}");
        s.history.heard(&t);
    }
}

/// Up Next before anything was heard: no History, so no pill and no wide fade.
fn up_next_no_history(s: &mut State) {
    up_next(s);
    s.history = Default::default();
}

fn now_playing_queue_long_history(s: &mut State) {
    up_next_long_history(s);
    s.settings.right_panel = false;
    s.settings.now_playing_queue = true;
    s.ui.now_playing = true;
}

fn now_playing_queue_no_history(s: &mut State) {
    now_playing_queue(s);
    s.history = Default::default();
}

/// The queue button with nothing after the playing song.
fn now_playing_queue_empty(s: &mut State) {
    now_playing(s);
    s.queue = PlayQueue::default();
    s.settings.now_playing_queue = true;
}

fn right_panel_lyrics(s: &mut State) {
    now_playing_base(s);
    s.ui.now_playing = false;
    s.playback.state = PlayState::Playing;
    s.playback.position = 33.0; // line 5 is current
    s.lyrics.insert("cat-now".into(), Load::Ready(Some(fake_lyrics())));
    s.settings.right_panel = true;
    s.settings.right_tab = crate::settings::RightTab::Lyrics;
}

fn new_playlist(s: &mut State) {
    playlist_page(s);
    let songs: Vec<Song> = (0..3).map(|k| song(2, k + 1)).collect();
    crate::ui::new_playlist_open(s, songs);
}

/// The New dialog with no songs: the name, description and picture part.
fn new_playlist_picture(s: &mut State) {
    playlist_page(s);
    crate::ui::new_playlist_open(s, Vec::new());
}

/// Edit on one of the user's own playlists, with a description.
fn edit_playlist(s: &mut State) {
    playlist_page(s);
    let mut c = s.library_playlists.items.first().cloned().expect("scene playlists");
    c.description = Some("Gentle starts for early trains.".into());
    crate::ui::edit_playlist_open(s, &c);
}

/// Edit on Favourite Songs, which can't be renamed: only the picture.
fn edit_playlist_picture_only(s: &mut State) {
    playlist_page(s);
    let c = s.library_playlists.items.iter().find(|c| c.favorites).cloned().expect("scene favourites");
    crate::ui::edit_playlist_open(s, &c);
}

fn settings_keyboard(s: &mut State) {
    home(s);
    s.ui.settings.open = Some(crate::ui::settings::Tab::Keyboard);
}

fn selection(s: &mut State) {
    playlist_page(s);
    crate::ui::scene_select(s, &["pl-4", "pl-5", "pl-6"], None);
}

fn context_menu(s: &mut State) {
    playlist_page(s);
    crate::ui::scene_select(s, &["pl-4", "pl-5", "pl-6"], Some([420.0, 330.0]));
}

fn signed_out(s: &mut State) {
    s.session.bridge_up = true;
    s.session.authorized = false;
}

fn settings_account(s: &mut State) {
    home(s);
    s.session.storefront = Some("it".into());
    s.settings.storefront = Some(crate::settings::Storefront { id: "it".into(), name: Some("Italy".into()) });
    s.ui.settings.open = Some(crate::ui::settings::Tab::Account);
}

/// Settings with its search field empty (the normal pages).
fn settings_search_empty(s: &mut State) {
    settings_account(s);
}

/// "lyrics": rows on Playback and Keyboard.
fn settings_search(s: &mut State) {
    settings_account(s);
    crate::ui::settings::set_query(s, "lyrics");
}

/// Settings search for "version": Check for updates and About (the version).
fn settings_about(s: &mut State) {
    settings_account(s);
    crate::ui::settings::set_query(s, "version");
}

fn settings_search_none(s: &mut State) {
    settings_account(s);
    crate::ui::settings::set_query(s, "xylophone");
}

/// After Enter on "tray": General, with the tray row tinted.
fn settings_search_flash(s: &mut State) {
    settings_general(s);
    s.ui.settings.flash = Some(crate::ui::settings::Row::Tray);
}

fn settings_signed_out(s: &mut State) {
    home(s);
    s.session.authorized = false;
    s.ui.settings.open = Some(crate::ui::settings::Tab::Account);
}

fn settings_sign_out_confirm(s: &mut State) {
    settings_account(s);
    s.ui.settings.confirm = true;
}

fn settings_playback(s: &mut State) {
    home(s);
    s.settings.data_saver = true;
    s.ui.settings.open = Some(crate::ui::settings::Tab::Playback);
}

/// Local only mode: Playback holds only Resume where you left off.
fn settings_playback_local(s: &mut State) {
    local_files(s);
    s.set_music_source(MusicSource::LocalOnly);
    s.ui.settings.open = Some(crate::ui::settings::Tab::Playback);
}

/// General → Updates after a manual check (found via search, so it is on screen).
fn settings_check_now(s: &mut State) {
    settings_general(s);
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    s.update_check = crate::update::CheckStatus { running: false, last_check: Some(now - 3600), result: Some(crate::update::UP_TO_DATE.into()) };
    crate::ui::settings::set_query(s, "check now");
}

fn settings_check_now_running(s: &mut State) {
    settings_check_now(s);
    s.update_check.running = true;
    s.update_check.result = None;
}

/// "Check for Updates…" from a menu: up to date.
fn update_check_toast(s: &mut State) {
    home(s);
    s.manual_answer(Ok(crate::update::Latest { version: crate::VERSION.into(), ..Default::default() }), None, crate::update::Install::Direct);
}

fn update_check_failed(s: &mut State) {
    home(s);
    s.manual_answer(Err("offline".into()), None, crate::update::Install::Direct);
}

/// The last run's song back at start: paused at 2:13, with its album as the queue.
fn resume_restored(s: &mut State) {
    album(s);
    let list: Vec<Song> = (1..=8).map(|k| song(2, k)).collect();
    s.playback = crate::state::Playback::default(); // the album scene has a song playing
    let snap = crate::resume::Snapshot::new(list[2].clone(), 133.0, &list, &list, 2, vec![song(1, 4)], false, "Album".into(), false, vec![song(1, 1)]);
    s.restore_playback(snap);
    s.settings.right_panel = true;
    s.settings.right_tab = crate::settings::RightTab::Queue;
}

fn settings_general(s: &mut State) {
    home(s);
    s.settings.close_to_tray = true;
    s.settings.tray_icon = true;
    s.cache_sizes = Some(crate::state::CacheSizes { art: 123 * 1024 * 1024, library: 4_404_019 });
    s.ui.settings.open = Some(crate::ui::settings::Tab::General);
}

/// The update banner (`update.rs`) over Home: a hand install (What's new, Download, Skip, ✕).
fn update_notice(s: &mut State) {
    update_scene(s, crate::update::Install::Direct);
}

/// Installed by pacman/AUR: "Update with your package manager", no Download.
fn update_notice_package(s: &mut State) {
    update_scene(s, crate::update::Install::System);
}

/// Installed by the Homebrew cask.
fn update_notice_homebrew(s: &mut State) {
    update_scene(s, crate::update::Install::Homebrew);
}

/// Built for the AUR (`UZUMAKI_CHANNEL=aur`): the package manager's command, no Download.
fn update_notice_aur(s: &mut State) {
    update_scene(s, crate::update::Install::Aur);
}

/// This version is below `minimum_supported`: the warning without Skip or ✕.
fn update_required(s: &mut State) {
    update_scene_with(s, crate::update::Install::Direct, Some("99.0.0"));
}

/// The same for the AUR build.
fn update_required_aur(s: &mut State) {
    update_scene_with(s, crate::update::Install::Aur, Some("99.0.0"));
}

fn update_scene(s: &mut State, install: crate::update::Install) {
    update_scene_with(s, install, None);
}

fn update_scene_with(s: &mut State, install: crate::update::Install, minimum: Option<&str>) {
    home(s);
    let latest = crate::update::Latest {
        version: "0.2.0".into(),
        build: 1,
        date: "2026-12-01".into(),
        notes_url: "https://github.com/hanami117/uzumaki/releases/tag/v0.2.0".into(),
        downloads: crate::update::Downloads {
            macos_dmg: "https://github.com/hanami117/uzumaki/releases/download/v0.2.0/Uzumaki-0.2.0.dmg".into(),
            linux_tarball: "https://github.com/hanami117/uzumaki/releases/download/v0.2.0/uzumaki-0.2.0.tar.gz".into(),
            ..Default::default()
        },
        minimum_supported: minimum.map(str::to_string),
    };
    let rec = crate::update::Record { last_check: Some(0), latest: Some(latest), ..Default::default() };
    s.update_known(&rec, install);
}

fn starting(_s: &mut State) {}

fn error(s: &mut State) {
    s.session.error = Some("Widevine (DRM) could not be installed: network unreachable".into());
    s.toast = Some("Could not play this song.".into());
}

// ---- sidebar, browse pages, search, menus ----

fn station(i: usize, name: &str, live: bool) -> Station {
    Station { id: format!("ra.{i}"), name: name.into(), artwork: None, live }
}

fn shelf(title: &str, items: Vec<Collection>, songs: Vec<Song>, stations: Vec<Station>) -> Shelf {
    Shelf { title: title.into(), items, songs, stations }
}

fn albums(idx: &[usize]) -> Vec<Collection> {
    idx.iter().map(|&i| album_item(i, false)).collect()
}

fn browse_shelves() -> Vec<Shelf> {
    vec![
        shelf("Fresh Picks", albums(&[2, 6, 0, 7, 4, 1, 5, 3, 9, 14]), vec![], vec![]),
        shelf("Top Songs", vec![], (0..16).map(|i| song(i % 8, (i / 8) as u32 + 1)).collect(), vec![]),
        shelf(
            "Stations for You",
            vec![],
            vec![],
            vec![station(1, "Chill Mix", false), station(2, "Focus Station", false), station(3, "Night Drive", false), station(4, "Indie Rock", false), station(5, "Jazz Standards", false), station(6, "Deep Cuts", false)],
        ),
        shelf("Editors' Albums", albums(&[7, 2, 5, 3, 0, 1, 4, 6]), vec![], vec![]),
    ]
}

fn new(s: &mut State) {
    base(s);
    s.route = Route::New;
    s.browse_new = Load::Ready(browse_shelves());
}

fn made_for_you(s: &mut State) {
    base(s);
    s.route = Route::MadeForYou;
    s.made_for_you = Load::Ready(browse_shelves().into_iter().rev().collect());
}

fn radio(s: &mut State) {
    base(s);
    s.route = Route::Radio;
    let live = vec![
        station(10, "Apple Music Country", true),
        station(11, "Apple Music Hits", true),
        station(12, "Apple Music 1", true),
        station(13, "Apple Music Club", true),
        station(14, "Apple Music Chill", true),
    ];
    s.radio[0] = Load::Ready(vec![shelf("Live", vec![], vec![], live)]);
    s.radio[1] = Load::Ready(vec![shelf("Your station", vec![], vec![], vec![station(20, "Mana's Station", false)])]);
}

fn recently_added(s: &mut State) {
    base(s);
    s.route = Route::RecentlyAdded;
    s.recently_added = Paged::ready((0..30).map(|i| album_item(i, false)).collect());
}

const ARTIST_A: [&str; 26] = [
    "Amber", "Black", "Cold", "Dawn", "\u{c9}lan", "Faded", "Glass", "Hollow", "Iron", "Jade", "Kilo", "Lunar", "Mono", "Night", "Opal",
    "Pale", "Quiet", "Red", "Salt", "Tidal", "Ultra", "Velvet", "Wild", "Xeno", "Yellow", "Zero",
];
const ARTIST_B: [&str; 10] = ["Owls", "Harbor", "Parade", "Club", "Machine", "Society", "Orchestra", "Echoes", "Kids", "Choir"];

fn fake_artists(n: usize) -> Vec<Artist> {
    (0..n)
        .map(|i| {
            let suffix = ["", " Jr", " Trio", " Quartet", " Project"][(i / 260) % 5];
            let name = format!("{} {}{suffix}", ARTIST_A[i % 26], ARTIST_B[(i / 26) % 10]);
            // A few single names.
            let name = if i % 97 == 0 { ARTIST_A[i % 26].to_string() + suffix } else { name };
            Artist { id: format!("ar{i}"), name }
        })
        .collect()
}

fn artists(s: &mut State) {
    base(s);
    s.route = Route::Artists;
    s.library_artists = Paged::ready(fake_artists(1300));
}

fn artist(s: &mut State) {
    base(s);
    s.route = Route::Artist { id: "ar12".into(), name: "Mara Quell".into() };
    s.back_stack.push(Route::Artists);
    s.artist_albums.insert("ar12".into(), Load::Ready(albums(&[0, 8, 16, 24, 32, 40, 1])));
}

/// A catalog artist page from fixture JSON shaped like Apple's reply (`app/fixtures/artist-full.json`):
/// sections of different lengths, an empty one (Live Albums), more pages behind Top Songs and Albums.
pub fn artist_fixture() -> crate::artist::ArtistPage {
    let v: serde_json::Value = serde_json::from_str(include_str!("../fixtures/artist-full.json")).expect("fixture");
    crate::artist::ArtistPage::parse(&v).expect("artist")
}

fn artist_full(s: &mut State) {
    base(s);
    s.route = Route::Artist { id: "28673423".into(), name: "Mara Quell".into() };
    s.back_stack.push(Route::Artists);
    s.artist_pages.insert("28673423".into(), Load::Ready(Some(artist_fixture())));
}

/// No editorial picture: the round portrait header.
fn artist_full_portrait(s: &mut State) {
    artist_full(s);
    if let Some(Load::Ready(Some(p))) = s.artist_pages.get_mut("28673423") {
        p.hero = None;
    }
}

/// With a background picture behind the page.
fn artist_full_bg(s: &mut State) {
    artist_full(s);
    builtin_bg(s, crate::appearance::Effect::Halftone);
}

fn artist_see_all(s: &mut State) {
    artist_full(s);
    s.back_stack.push(s.route.clone());
    s.route = Route::ArtistShelf { id: "28673423".into(), name: "Mara Quell".into(), shelf: crate::artist::Shelf::Singles };
}

/// A library artist Apple has no catalog match for: the old albums grid.
fn artist_library_fallback(s: &mut State) {
    base(s);
    s.route = Route::Artist { id: "r.12".into(), name: "Mara Quell".into() };
    s.back_stack.push(Route::Artists);
    s.artist_pages.insert("r.12".into(), Load::Ready(None));
    s.artist_albums.insert("r.12".into(), Load::Ready(albums(&[0, 8, 16, 24, 32, 40, 1])));
}

fn library_fixture(s: &mut State) {
    s.library_songs = Paged::ready((0..2000).map(|i| song(i % 8, (i / 8) as u32 % 9)).collect());
    s.library_albums = Paged::ready((0..64).map(|i| album_item(i, false)).collect());
    let mut artists = fake_artists(300);
    artists.push(Artist { id: "ar-n1".into(), name: "Night Owls".into() });
    artists.push(Artist { id: "ar-n2".into(), name: "Nightingale Row".into() });
    s.library_artists = Paged::ready(artists);
    let mut pls = s.library_playlists.items.clone();
    pls.push(playlist_item(5, "Night shift: city lights"));
    s.library_playlists = Paged::ready(pls);
}

fn search_library(s: &mut State) {
    base(s);
    library_fixture(s);
    s.route = Route::Search;
    s.ui.search_tab = crate::ui::SearchTab::Library;
    s.ui.search.text = "night".into();
}

fn search_apple(s: &mut State) {
    search(s);
    s.ui.search_tab = crate::ui::SearchTab::Apple;
    s.ui.search.text = "night".into();
}

/// The Search page over a picture background (Plain, the strengths of the defaults), for the
/// field's caret and selection colours on a tinted page.
fn search_bg(s: &mut State) {
    search(s);
    picture_bg(s, crate::appearance::Effect::Plain);
    let a = &mut s.settings.appearance;
    (a.opacity, a.list_opacity, a.light_opacity, a.light_list_opacity) = (0.65, 0.65, 0.9, 0.9);
}

fn pins() -> Vec<Pin> {
    let pin = |kind, id: &str, title: &str| Pin { kind, id: id.into(), title: title.into(), artwork: None };
    vec![
        pin(PinKind::Album, "a2", "Night Bus Hymns"),
        pin(PinKind::Artist, "ar12", "Mara Quell"),
        pin(PinKind::Station, "ra.3", "Night Drive"),
    ]
}

fn many_playlists(s: &mut State) {
    let mut lists = s.library_playlists.items.clone();
    for i in 0..40 {
        let mut c = playlist_item(100 + i, &format!("Playlist {}", ["Roadtrip", "Gym", "Rainy day", "Study", "Dinner"][i % 5]));
        c.editable = true;
        lists.push(c);
    }
    s.library_playlists = Paged::ready(lists);
}

fn sidebar_pins(s: &mut State) {
    playlist_page(s);
    many_playlists(s);
    s.settings.pins = pins();
}

/// The sidebar's playlists, some with a picture (a thumbnail like a pin's) and some without (the
/// icon), Favourite Songs among them. No cover loads headless, so the thumbnails show as tinted
/// tiles with initials.
fn sidebar_playlist_art(s: &mut State) {
    sidebar_pins(s);
    let mut lists = s.library_playlists.items.clone();
    for (i, c) in lists.iter_mut().enumerate() {
        if i % 2 == 0 {
            c.artwork = Some(Artwork { template: format!("https://scene.invalid/pl{i}/{{w}}x{{h}}bb.jpg"), bg: None });
        }
    }
    s.library_playlists = Paged::ready(lists);
    s.settings.pins[0].artwork = Some(Artwork { template: "https://scene.invalid/pin/{w}x{h}bb.jpg".into(), bg: None });
}

/// Pin 3 lifted over the top of the group, its neighbours stepped aside.
fn drag_pins(s: &mut State) {
    sidebar_pins(s);
    crate::ui::scene_drag_pin(s, 2, 0, -62.0);
}

/// Two selected songs dragged over an editable playlist in the sidebar.
fn drag_songs(s: &mut State) {
    sidebar_pins(s);
    crate::ui::scene_select(s, &["pl-4", "pl-5"], None);
    crate::ui::scene_drag_songs(s, [70.0, 706.0], Some("p.1"));
}

fn sidebar_collapsed(s: &mut State) {
    sidebar_pins(s);
    s.settings.sidebar_collapsed = true;
}

/// The rail with the pins folded, so its playlists show: pictures (tinted tiles with initials,
/// headless) and icons, "Morning commute" open.
fn sidebar_collapsed_playlists(s: &mut State) {
    sidebar_playlist_art(s);
    s.settings.sidebar_collapsed = true;
    s.settings.pins_open = false;
}

/// Two songs dragged over the rail's second playlist.
fn drag_songs_rail(s: &mut State) {
    sidebar_collapsed_playlists(s);
    crate::ui::scene_select(s, &["pl-4", "pl-5"], None);
    crate::ui::scene_drag_songs(s, [38.0, 805.0], Some("p.1"));
}

/// Meant for `--size 900x700`.
fn narrow(s: &mut State) {
    base(s);
    s.settings.pins = pins();
    s.route = Route::New;
    s.browse_new = Load::Ready(browse_shelves());
}

/// Meant for `--size 700x600`.
fn compact(s: &mut State) {
    base(s);
    s.route = Route::Library(LibraryTab::Albums);
    s.library_albums = Paged::ready((0..24).map(|i| album_item(i, false)).collect());
}

fn menu_pin(s: &mut State) {
    sidebar_pins(s);
    crate::ui::scene_menu_pin(s, "ar12", [60.0, 300.0]);
}

/// A pinned library playlist's menu: the playlist row's items, then Unpin and Move Up/Down.
fn menu_pin_playlist(s: &mut State) {
    sidebar_pins(s);
    let c = s.library_playlists.items.iter().find(|c| c.editable).cloned().expect("an editable playlist");
    s.settings.pins.insert(1, Pin { kind: PinKind::Playlist, id: c.id.clone(), title: c.title.clone(), artwork: None });
    crate::ui::scene_menu_pin(s, &c.id, [60.0, 260.0]);
}

/// The same menu from the collapsed rail, on a pinned album.
fn menu_pin_rail(s: &mut State) {
    sidebar_collapsed(s);
    crate::ui::scene_menu_pin(s, "a2", [40.0, 200.0]);
}

fn menu_card(s: &mut State) {
    recently_added(s);
    let c = album_item(1, false);
    s.library_albums = Paged::ready(vec![]);
    crate::ui::scene_menu_collection(s, &c, [520.0, 260.0]);
}

// ---- Share ▸ Copy Link ----

/// Opens the Share submenu of the context or song menu that is showing.
fn share_open(s: &mut State) {
    crate::ui::scene_share_open(s);
}

/// A catalog album card (as Apple Music's: Share ▸ Copy Link in the "…" menu).
fn share_album_card(s: &mut State) {
    recently_added(s);
    let mut c = album_item(1, false);
    c.id = "617154241".into();
    s.session.storefront = Some("it".into());
    crate::ui::scene_menu_collection(s, &c, [520.0, 200.0]);
    share_open(s);
}

/// The album page's "…" menu; the page's own `attributes.url` is preferred over a built one.
fn share_album_header(s: &mut State) {
    album(s);
    s.session.storefront = Some("it".into());
    let id = "a2".to_string();
    crate::share::note(
        crate::share::Kind::Album,
        &serde_json::json!({"id": id, "attributes": {"url": "https://music.apple.com/it/album/aurora-line/1600000002"}}),
    );
    if let Some(Load::Ready(c)) = s.collections.get(&id).cloned() {
        crate::ui::scene_menu_collection(s, &c, [770.0, 300.0]);
        share_open(s);
    }
}

fn share_playlist(s: &mut State) {
    recently_added(s);
    let mut c = playlist_item(7, "Today's Hits");
    c.id = "pl.f4d106fed2bd41149aaacabb233eb5eb".into();
    s.session.storefront = Some("it".into());
    crate::ui::scene_menu_collection(s, &c, [520.0, 200.0]);
    share_open(s);
}

/// The user's own library playlist has no catalog id: no Share in its menu.
fn share_playlist_library_hidden(s: &mut State) {
    recently_added(s);
    let c = s.library_playlists.items.iter().find(|c| c.editable).cloned().expect("an editable playlist");
    crate::ui::scene_menu_collection(s, &c, [520.0, 200.0]);
}

fn share_artist(s: &mut State) {
    artist(s);
    s.session.storefront = Some("it".into());
    let a = Artist { id: "5468295".into(), name: "Mara Quell".into() };
    crate::ui::scene_menu_artist(s, &a, [300.0, 200.0]);
    share_open(s);
}

fn share_station(s: &mut State) {
    radio(s);
    s.session.storefront = Some("it".into());
    crate::ui::scene_menu_station(s, &station(978194965, "Apple Music 1", true), [520.0, 240.0]);
    share_open(s);
}

fn catalog_song(s: &mut State) -> Song {
    let mut now = song(2, 1);
    now.id = "1600000010".into();
    now.catalog_id = Some("1600000010".into());
    now.album_id = Some("1600000002".into());
    s.session.storefront = Some("it".into());
    now
}

/// Right-click on the player bar: Share ▸ Copy Link of the playing song.
fn share_song_player_bar(s: &mut State) {
    home(s);
    let now = catalog_song(s);
    s.playback.song = Some(now.clone());
    crate::ui::scene_song_menu(s, now, [80.0, 380.0]);
    share_open(s);
}

fn share_song_row(s: &mut State) {
    playlist_page(s);
    let now = catalog_song(s);
    crate::ui::scene_song_menu(s, now, [420.0, 240.0]);
    share_open(s);
}

/// A library album: its catalog page is asked for when Copy Link is clicked.
fn share_library_album(s: &mut State) {
    library_albums(s);
    let mut c = album_item(1, false);
    c.id = "l.8f3a1".into();
    crate::ui::scene_menu_collection(s, &c, [520.0, 200.0]);
    share_open(s);
}

fn menu_submenu(s: &mut State) {
    context_menu(s);
    crate::ui::scene_submenu(s);
}

// ---- local files ----

// (artist, album, year, [(title, seconds)])
const LOCAL_ALBUMS: [(&str, &str, &str, [(&str, u32); 4]); 3] = [
    ("Mara Quell", "Low Tide Radio", "2025", [("Harbor", 212), ("Night Signal", 187), ("Under Neon", 243), ("Small Rooms", 176)]),
    ("Telu", "Static Garden", "2023", [("Driftwood", 301), ("Late Train", 154), ("Afterglow", 228), ("Copper Sky", 199)]),
    ("Ines Rook", "Paper Satellites", "2025", [("Feral", 165), ("Quiet Engine", 258), ("Night Halflight", 190), ("Overpass", 233)]),
];

fn local_songs() -> Vec<Song> {
    let mut v = Vec::new();
    for round in 0..4 {
        for (artist, album, year, tracks) in LOCAL_ALBUMS {
            let album = if round == 0 { album.to_string() } else { format!("{album} (Disc {})", round + 1) };
            for (n, (title, secs)) in tracks.iter().enumerate() {
                if v.len() >= 40 {
                    break;
                }
                let n = n as u32 + 1;
                v.push(Song {
                    id: format!("file:///music/{artist}/{album}/{n:02} {title}.flac"),
                    catalog_id: None,
                    title: title.to_string(),
                    artist: artist.into(),
                    album: album.clone(),
                    album_is_fallback: false,
                    duration: (*secs + round * 7) as f64,
                    track_number: Some(n),
                    disc_number: Some(1),
                    explicit: false,
                    artwork: None,
                    playable: true,
                    genre: None,
                    year: Some(year.into()),
                    album_id: None,
                    artists: Vec::new(),
                });
            }
        }
    }
    v.truncate(40);
    v
}

pub(crate) fn home_dir() -> std::path::PathBuf {
    std::env::var_os("HOME").map(Into::into).unwrap_or_else(|| "/home/user".into())
}

fn local_files(s: &mut State) {
    base(s);
    s.settings.music_folders = vec![home_dir().join("Music")];
    s.local_lib.songs = local_songs();
    s.local_lib.loaded = true;
    s.route = Route::LocalFiles;
}

fn local_files_empty(s: &mut State) {
    base(s);
    s.route = Route::LocalFiles;
}

fn local_files_scanning(s: &mut State) {
    base(s);
    s.settings.music_folders = vec![home_dir().join("Music")];
    s.local_lib.scanning = true;
    s.local_lib.scanned = 340;
    s.route = Route::LocalFiles;
}

fn settings_local(s: &mut State) {
    local_files(s);
    s.settings.music_folders = vec![home_dir().join("Music"), "/mnt/media/flac".into()];
    s.ui.settings.open = Some(crate::ui::settings::Tab::Local);
}

fn search_local(s: &mut State) {
    base(s);
    s.settings.music_folders = vec![home_dir().join("Music")];
    s.local_lib.songs = local_songs();
    s.local_lib.loaded = true;
    s.route = Route::Search;
    s.ui.search_tab = crate::ui::SearchTab::Local;
    s.ui.search.text = "night".into();
}

// ---- backgrounds (Settings -> Appearance) ----

/// The built-in swirl as a PNG, so a scene's song has artwork a headless run can read.
fn scene_cover() -> String {
    // `$UZUMAKI_SCENE_COVER` (any PNG/JPEG) stands in for the swirl, like `$UZUMAKI_SCENE_PICTURE`.
    if let Some(p) = std::env::var_os("UZUMAKI_SCENE_COVER").filter(|p| !p.is_empty()) {
        return format!("file://{}", std::path::Path::new(&p).display());
    }
    let path = save_temp_png("uzumaki-scene-cover.png", &uzumaki_fx::builtin(600, 600));
    format!("file://{}", path.display())
}

/// The background with the right panel open: the picture stays window-anchored (no re-crop).
fn bg_right_panel(s: &mut State) {
    up_next(s);
    builtin_bg(s, crate::appearance::Effect::Dither);
}

/// A 2560×1440 screen: the picture is rendered at that size and centred, so snaps at two window
/// sizes show the same scale, cropped differently.
fn bg_wallpaper(s: &mut State) {
    crate::backdrop::set_screen([2560.0, 1440.0]);
    home(s);
    builtin_bg(s, crate::appearance::Effect::Halftone);
    s.settings.appearance.fade = false;
    s.settings.appearance.opacity = 0.65;
}

fn builtin_bg(s: &mut State, effect: crate::appearance::Effect) {
    let a = &mut s.settings.appearance;
    a.source = crate::appearance::BgSource::Builtin;
    a.effect = effect;
}

// ---- first-run onboarding ----

use crate::settings::MusicSource;
use crate::ui::onboarding::{Mode, Step};

/// A first run from the start pose, running: render it with `--frames N --out DIR` (a script
/// can tap `onb/start`, `onb/choice/local`, `onb/back`, `onb/skip`).
fn onboarding(s: &mut State) {
    home(s);
    crate::ui::onboarding::start(s);
}

/// "Show the tour again" from Settings, running over the app.
fn onboarding_replay(s: &mut State) {
    home(s);
    s.settings.onboarded = Some(crate::ui::onboarding::TOUR_VERSION);
    crate::ui::onboarding::replay(s);
}

/// The onboarding frozen on one step.
fn onboarding_at(s: &mut State, mode: Mode, step: Step, chosen: Option<MusicSource>) {
    home(s);
    crate::ui::onboarding::scene(s, mode, step, chosen);
}

/// The Folders step (local files only). `n`: 0 none (no suggestion either), 1 the suggested
/// ~/Music while it scans, 2 ~/Music and a second folder with a count, 3 a second folder with the
/// suggestion unticked.
fn onboarding_folders(s: &mut State, n: usize, scanning: bool) {
    onboarding_at(s, Mode::FirstRun, Step::Folders, Some(MusicSource::LocalOnly));
    s.settings.music_source = MusicSource::LocalOnly;
    let music = home_dir().join("Music");
    if n > 0 {
        crate::ui::onboarding::scene_suggest(s, music.clone());
    }
    s.settings.music_folders = match n {
        0 => vec![],
        1 => vec![music],
        2 => vec![music, "/mnt/media/flac".into()],
        _ => vec!["/mnt/media/flac".into()],
    };
    if scanning {
        s.local_lib.scanning = true;
        s.local_lib.scanned = 340;
    } else if n > 0 {
        s.local_lib.songs = (0..1240).map(|i| local_songs()[i % 40].clone()).collect();
        s.local_lib.loaded = true;
    }
}

/// The Sign in step (Apple Music). `n`: 0 the engine starting, 1 the lead-in, 2 waiting for
/// Apple's window, 3 the window closed, 4 signed in (store Italy, the library syncing behind),
/// 5 signed in before the store is known, 6 the engine failed.
fn onboarding_sign_in(s: &mut State, n: usize) {
    use crate::ui::onboarding::SignIn;
    onboarding_at(s, Mode::FirstRun, Step::SignIn, Some(MusicSource::Apple));
    s.session.bridge_up = n != 0;
    s.session.authorized = false;
    match n {
        2 => {
            crate::ui::onboarding::scene_sign_in(s, SignIn::Waiting);
            s.session.login_window = true;
        }
        3 => crate::ui::onboarding::scene_sign_in(s, SignIn::Closed),
        4 | 5 => {
            s.session.authorized = true;
            if n == 4 {
                s.settings.storefront = Some(crate::settings::Storefront { id: "it".into(), name: Some("Italy".into()) });
            }
            syncing_library(s);
        }
        6 => s.session.error = Some("Widevine (DRM) could not be installed: download failed (net::ERR_INTERNET_DISCONNECTED)".into()),
        _ => {}
    }
}

/// The library halfway down, behind the onboarding (no sync screen while it is up).
fn syncing_library(s: &mut State) {
    s.syncing = true;
    s.library_songs.items = (0..1234).map(|i| song(i % 8, (i % 10) as u32)).collect();
    s.library_songs.load = Load::Loading;
    s.library_songs.total = Some(3312);
    s.library_albums.load = Load::Loading;
    s.library_playlists.load = Load::Loading;
}

/// Settings → Account → Apple Music while signed out: the Sign in step on its own (no dots, no
/// Back or Skip).
fn onboarding_sign_in_only(s: &mut State) {
    onboarding_at(s, Mode::SignIn, Step::SignIn, None);
    s.session.bridge_up = true;
    s.session.authorized = false;
}

/// Done after signing in, the library still syncing behind (the line at the bottom).
fn onboarding_done_syncing(s: &mut State) {
    onboarding_at(s, Mode::FirstRun, Step::Done, Some(MusicSource::Apple));
    s.session.bridge_up = true;
    s.session.authorized = true;
    syncing_library(s);
}

/// The shortcuts tour on card `i` (`ui/tour.rs` order: play, sidebar, right, panels,
/// now-playing, find, skip, more), Apple Music or local only, with these shortcuts pressed
/// (`Action::id`s, "slotN" for a number key).
fn onboarding_tour(s: &mut State, local: bool, i: usize, pressed: &[&str]) {
    let source = if local { MusicSource::LocalOnly } else { MusicSource::Apple };
    onboarding_at(s, Mode::FirstRun, Step::Tour, Some(source));
    s.settings.music_source = source;
    if !local {
        s.session.bridge_up = true;
        s.session.authorized = true;
    }
    crate::ui::onboarding::scene_tour(s, i, pressed);
}

/// The user's own shortcuts: the sidebar on Alt+S, play/pause removed (its card is left out),
/// so the first card is the sidebar's.
fn onboarding_tour_rebound(s: &mut State) {
    s.settings.keybinds.insert("toggle_sidebar".into(), "alt+s".into());
    s.settings.keybinds.insert("play_pause".into(), String::new());
    onboarding_tour(s, false, 0, &[]);
}

/// The closing card with the number keys on Alt and opening pins.
fn onboarding_tour_pins(s: &mut State) {
    s.settings.number_mod = crate::settings::NumberMod::Alt;
    s.settings.number_keys = crate::settings::NumberKeys::Pins;
    for (id, title) in [("p1", "Late Night Drive"), ("a1", "Telu")] {
        s.settings.pins.push(crate::settings::Pin { kind: crate::settings::PinKind::Playlist, id: id.into(), title: title.into(), artwork: None });
    }
    onboarding_tour(s, false, 7, &[]);
}

/// "Show the tour again": its shortcuts tour (three dots: Welcome, Tour, Done).
fn onboarding_replay_tour(s: &mut State) {
    onboarding_at(s, Mode::Replay, Step::Tour, None);
    s.session.bridge_up = true;
    s.session.authorized = true;
}

// ---- local only mode ----

/// Local only: Local Files with its songs; the sidebar keeps Search and Local Files only, though
/// the library underneath has Apple playlists and pins.
fn local_only_sidebar(s: &mut State) {
    local_files(s);
    s.set_music_source(MusicSource::LocalOnly);
}

fn local_only_rail(s: &mut State) {
    local_only_sidebar(s);
    s.settings.sidebar_collapsed = true;
}

/// No folders yet: the empty page with its Add a music folder button, and no banners.
fn local_only_empty(s: &mut State) {
    local_files_empty(s);
    s.session.bridge_up = false;
    s.session.authorized = false;
    s.set_music_source(MusicSource::LocalOnly);
}

/// Search in local only mode: the music folders, no tabs.
fn local_only_search(s: &mut State) {
    search_local(s);
    s.set_music_source(MusicSource::LocalOnly);
    s.route = Route::Search;
}

/// Settings → Account: the music source switch above the Apple Music sign-in.
fn settings_music_source(s: &mut State) {
    settings_account(s);
}

/// The same in local only mode: no sign-in row, no Playback page.
fn settings_music_source_local(s: &mut State) {
    local_files(s);
    s.set_music_source(MusicSource::LocalOnly);
    s.ui.settings.open = Some(crate::ui::settings::Tab::Account);
}

// ---- startup intro ----

/// The short intro over Home, running: render it with `--frames N --out DIR`.
fn intro(s: &mut State) {
    home(s);
    s.ui.intro = crate::ui::intro::at_launch(true, false);
}

/// The first-sync screen, frozen mid-download.
fn intro_sync(s: &mut State) {
    home(s);
    s.session.bridge_up = true;
    s.session.authorized = true;
    s.library_songs.items = (0..1234).map(|i| song(i % 8, (i % 10) as u32)).collect();
    s.library_songs.load = Load::Loading;
    s.library_albums.items = (0..210).map(|i| album_item(i % 40, false)).collect();
    s.library_albums.load = Load::Loading;
    s.library_playlists = Paged::ready((0..35).map(|i| album_item(i % 40, false)).collect());
    s.library_songs.total = Some(3312);
    s.library_albums.total = Some(2104);
    crate::ui::intro::scene(s, crate::ui::intro::Kind::Sync, crate::ui::intro::Phase::In);
}

/// The sync screen was skipped: the download goes on behind the app, with a progress pill.
fn sync_pill(s: &mut State) {
    intro_sync(s);
    s.ui.intro = None;
    s.syncing = true;
}

fn sync_pill_floating(s: &mut State) {
    sync_pill(s);
    s.settings.player_style = crate::settings::PlayerStyle::Floating;
}

/// The background sync just finished.
fn sync_pill_done(s: &mut State) {
    sync_pill(s);
    s.syncing = false;
    s.library_songs = Paged::ready(s.library_songs.items.clone());
    s.library_albums = Paged::ready(s.library_albums.items.clone());
    s.ui.sync_done = Some(0.0);
}

/// The first-sync screen while Electron starts.
fn intro_starting(s: &mut State) {
    home(s);
    s.session.bridge_up = false;
    crate::ui::intro::scene(s, crate::ui::intro::Kind::Sync, crate::ui::intro::Phase::In);
}

fn appearance(s: &mut State) {
    home(s);
    builtin_bg(s, crate::appearance::Effect::Dither);
    s.ui.settings.open = Some(crate::ui::settings::Tab::Appearance);
}

/// Settings → Appearance with App icon → Follow theme chosen.
fn appearance_icon_auto(s: &mut State) {
    appearance(s);
    s.set_app_icon(crate::settings::AppIcon::Auto);
}

/// A chosen picture: "Change…" and "Remove" show.
fn appearance_picture(s: &mut State) {
    home(s);
    let path = save_temp_png("uzumaki-scene-picture.png", &uzumaki_fx::builtin(800, 500));
    s.settings.appearance.picture = Some(path);
    s.settings.appearance.source = crate::appearance::BgSource::Picture;
    s.settings.appearance.effect = crate::appearance::Effect::Halftone;
    s.ui.settings.open = Some(crate::ui::settings::Tab::Appearance);
}

/// An album page with the background on card pages only: no picture here.
fn bg_list_cards_only(s: &mut State) {
    album(s);
    builtin_bg(s, crate::appearance::Effect::Dither);
    s.settings.appearance.show_on = crate::appearance::ShowOn::Cards;
}

/// A song list with the song-list strength at its faintest-but-visible 0.12.
fn bg_list_faint(s: &mut State) {
    library_songs(s);
    builtin_bg(s, crate::appearance::Effect::Dither);
    s.settings.appearance.list_opacity = 0.12;
}

fn bg_dither(s: &mut State) {
    home(s);
    builtin_bg(s, crate::appearance::Effect::Dither);
}

fn bg_ascii(s: &mut State) {
    home(s);
    builtin_bg(s, crate::appearance::Effect::Ascii);
}

fn bg_halftone(s: &mut State) {
    home(s);
    builtin_bg(s, crate::appearance::Effect::Halftone);
}

/// A photo-like picture for the Picture scenes: `$UZUMAKI_SCENE_PICTURE` if set (any PNG/JPEG),
/// else a generated landscape (sky, sun, dark hills, a red bridge-like band), so both themes'
/// handling of dark, bright and saturated parts shows.
fn scene_picture() -> std::path::PathBuf {
    if let Some(p) = std::env::var_os("UZUMAKI_SCENE_PICTURE").filter(|p| !p.is_empty()) {
        return p.into();
    }
    let (w, h) = (1200u32, 750u32);
    let img = image::RgbaImage::from_fn(w, h, |x, y| {
        let (u, v) = (x as f32 / w as f32, y as f32 / h as f32);
        let sky = [255.0 - 90.0 * v, 190.0 - 40.0 * v + 40.0 * u, 140.0 + 100.0 * u];
        let sun = (1.0 - ((u - 0.25).hypot(v - 0.35) / 0.12)).clamp(0.0, 1.0);
        let hill = 0.62 + 0.06 * (u * 9.0).sin() + 0.03 * (u * 23.0).cos();
        let band = (u > 0.62 && u < 0.68 && v > 0.15) || ((v - 0.5 - 0.25 * (u - 0.65)).abs() < 0.015 && u > 0.5);
        let c = if band {
            [196.0, 52.0, 40.0]
        } else if v > hill {
            let d = (v - hill) * 2.0;
            [40.0 + 30.0 * d, 46.0 + 20.0 * d, 58.0]
        } else {
            [sky[0] + (255.0 - sky[0]) * sun, sky[1] + (240.0 - sky[1]) * sun, sky[2] + (200.0 - sky[2]) * sun]
        };
        image::Rgba([c[0].clamp(0.0, 255.0) as u8, c[1].clamp(0.0, 255.0) as u8, c[2].clamp(0.0, 255.0) as u8, 255])
    });
    save_temp_png("uzumaki-scene-photo.png", &img)
}

/// Saves a scene's picture in the temp folder under `name`. It is written to a file of its own
/// and renamed into place, so a reader (a parallel test, another headless run) never sees half
/// a file while it is rewritten.
fn save_temp_png(name: &str, img: &image::RgbaImage) -> std::path::PathBuf {
    static WRITES: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let dir = std::env::temp_dir();
    let path = dir.join(name);
    let n = WRITES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp = dir.join(format!(".{name}.{}-{n}.part", std::process::id()));
    match img.save_with_format(&tmp, image::ImageFormat::Png) {
        Ok(()) => {
            let _ = std::fs::rename(&tmp, &path);
        }
        Err(_) => {
            let _ = std::fs::remove_file(&tmp);
        }
    }
    path
}

/// Home with a Picture background and `effect`, at typical settings (opacity and
/// list opacity 0.35, every page, fade on, Now Playing on the app's picture with Blur).
fn bg_picture(s: &mut State, effect: crate::appearance::Effect) {
    home(s);
    picture_bg(s, effect);
}

fn picture_bg(s: &mut State, effect: crate::appearance::Effect) {
    let a = &mut s.settings.appearance;
    a.source = crate::appearance::BgSource::Picture;
    a.picture = Some(scene_picture());
    a.effect = effect;
    a.opacity = 0.35;
    a.list_opacity = 0.35;
    a.show_on = crate::appearance::ShowOn::All;
    a.fade = true;
    a.np = crate::appearance::NpBg::App;
    a.np_effect = crate::appearance::Effect::Blur;
    // `$UZUMAKI_SCENE_EFFECT` (an `Effect` name, e.g. "Blur") and `$UZUMAKI_SCENE_OPACITY` (every
    // opacity, both themes) override the scene's, to compare looks and strengths.
    if let Some(e) = std::env::var("UZUMAKI_SCENE_EFFECT").ok().and_then(|v| serde_json::from_value(serde_json::Value::String(v)).ok()) {
        a.effect = e;
    }
    if let Some(v) = std::env::var("UZUMAKI_SCENE_OPACITY").ok().and_then(|v| v.parse::<f32>().ok()) {
        a.opacity = v;
        a.list_opacity = v;
        a.light_opacity = v;
        a.light_list_opacity = v;
    }
}

/// The Songs table of a playlist with long titles over the Picture + Halftone background, for
/// checking text contrast on a song list.
fn bg_picture_songs(s: &mut State) {
    playlist_songs(s);
    picture_bg(s, crate::appearance::Effect::Halftone);
    if let Some(Load::Ready(c)) = s.collections.values_mut().next() {
        for (i, t) in c.tracks.iter_mut().enumerate().filter(|(i, _)| i % 3 == 1) {
            t.title = format!("{} (Live at the Harbour Hall, Remastered {})", t.title, 2010 + i);
        }
    }
}

/// An album page (a song list) over the Picture + Dither background.
fn bg_picture_album(s: &mut State) {
    album(s);
    picture_bg(s, crate::appearance::Effect::Dither);
}

fn bg_blur(s: &mut State) {
    home(s);
    builtin_bg(s, crate::appearance::Effect::Blur);
}

/// Strongest opacity, no fade, on a long song list: any opaque panel shows as a patch.
fn bg_strong(s: &mut State) {
    library_songs(s);
    builtin_bg(s, crate::appearance::Effect::Plain);
    s.settings.appearance.opacity = crate::appearance::OPACITY_MAX;
    s.settings.appearance.fade = false;
}

fn np_bg_cover(s: &mut State) {
    np_bg_cover_with(s, crate::appearance::Effect::Blur);
}

/// Now Playing's "Cover" background with `effect` (the site demo's reference shots).
fn np_bg_cover_with(s: &mut State, effect: crate::appearance::Effect) {
    now_playing(s);
    if let Some(song) = &mut s.playback.song {
        song.artwork = Some(Artwork { template: scene_cover(), bg: Some("a79be6".into()) });
    }
    s.settings.appearance.np = crate::appearance::NpBg::Cover;
    s.settings.appearance.np_effect = effect;
}

fn np_bg_app(s: &mut State) {
    now_playing(s);
    builtin_bg(s, crate::appearance::Effect::Dither);
    s.settings.appearance.opacity = 0.5;
    s.settings.appearance.np = crate::appearance::NpBg::App;
}

/// The setup of the 2026-11-09 stutter screencast, Now Playing closed: an album (a song-list
/// page), the right panel on Lyrics, a Halftone picture behind the content and the same picture
/// behind Now Playing. Open and close it from a script (Ctrl+Shift+F, Esc) with `--frames`.
fn np_slide(s: &mut State) {
    np_bg_app(s);
    s.settings.appearance.effect = crate::appearance::Effect::Halftone;
    s.settings.appearance.show_on = crate::appearance::ShowOn::All;
    s.settings.right_panel = true;
    s.settings.right_tab = crate::settings::RightTab::Lyrics;
    s.ui.now_playing = false;
}

/// Settings → Keyboard customised: Alt+1–9 open pins, the sidebar on Alt+S, Next removed, and
/// "Previous song" waiting for its new keys.
fn settings_keyboard_edit(s: &mut State) {
    settings_keyboard(s);
    s.settings.number_mod = crate::settings::NumberMod::Alt;
    s.settings.number_keys = crate::settings::NumberKeys::Pins;
    s.settings.keybinds.insert("toggle_sidebar".into(), "alt+s".into());
    s.settings.keybinds.insert("next".into(), String::new());
    s.ui.recording = crate::ui::keys_index("previous");
}

/// Right-click on the player bar: the playing song's menu.
fn menu_now_playing(s: &mut State) {
    home(s);
    if let Some(song) = s.playback.song.clone() {
        crate::ui::scene_song_menu(s, song, [80.0, 520.0]);
    }
}

/// The player bar's speaker at each volume level (Home's own 0.7 shows three waves).
fn volume_muted(s: &mut State) {
    home(s);
    s.playback.volume = 0.0;
}

fn volume_low(s: &mut State) {
    home(s);
    s.playback.volume = 0.2;
}

fn volume_mid(s: &mut State) {
    home(s);
    s.playback.volume = 0.5;
}

/// Floating player bar over Home (also used for the narrow window).
fn player_floating(s: &mut State) {
    home(s);
    let mut now = song(2, 1);
    now.title = "Last Service".into();
    now.duration = 241.0;
    now.artwork = Some(Artwork { template: "https://example.invalid/{w}x{h}.{f}".into(), bg: Some("a79be6".into()) });
    s.playback.song = Some(now);
    s.playback.position = 75.0;
    s.playback.shuffle = true;
    s.settings.player_style = crate::settings::PlayerStyle::Floating;
}

/// Appearance → Sidebar in use: New, Radio and Made for You hidden.
fn sidebar_hidden(s: &mut State) {
    home(s);
    s.settings.hidden_nav = vec!["nav-new".into(), "nav-radio".into(), "nav-mfy".into()];
    crate::ui::settings::open_tab(s, crate::ui::settings::Tab::Appearance);
}

/// Settings → Appearance with a picture background (the app icon) and the halftone effect:
/// the preview tile shows it. (Headless renders backgrounds synchronously.)
fn appearance_preview(s: &mut State) {
    home(s);
    let ap = &mut s.settings.appearance;
    ap.source = crate::appearance::BgSource::Picture;
    ap.picture = Some(concat!(env!("CARGO_MANIFEST_DIR"), "/../assets/icons/uzumaki.png").into());
    ap.effect = crate::appearance::Effect::Halftone;
    crate::ui::settings::open_tab(s, crate::ui::settings::Tab::Appearance);
}

// ---- favourites (all signed in, via `base`) ----

/// A playlist as the Songs table, with some rows starred (the playing song, pl-3, is further down).
fn fav_table(s: &mut State) {
    table_scene(s);
    s.star_for_scene(&["pl-13", "pl-11", "pl-9", "pl-3"]);
}

/// An album page: the header star filled, and two tracks starred.
fn fav_album(s: &mut State) {
    album(s);
    s.star_for_scene(&["a2", "s-2-0", "s-2-2"]);
}

/// The song menu on a starred song, so it offers "Undo Favourite".
fn fav_menu(s: &mut State) {
    fav_table(s);
    let song = s.collections.get("p.0").and_then(|l| l.ready()).and_then(|c| c.tracks.get(9).cloned());
    if let Some(song) = song {
        crate::ui::scene_song_menu(s, song, [420.0, 300.0]);
    }
}

/// Up Next with stars: the playing song, a Play Next song, an upcoming one and one in History.
fn up_next_stars(s: &mut State) {
    up_next(s);
    s.star_for_scene(&["ctx-1", "ctx-2", "user-1", "ctx-5"]);
}

/// The same in Now Playing's Up Next.
fn now_playing_queue_stars(s: &mut State) {
    now_playing_queue(s);
    s.star_for_scene(&["ctx-1", "ctx-2", "user-1", "ctx-5"]);
}

/// The card menu of a favourite album.
fn fav_card_menu(s: &mut State) {
    s.star_for_scene(&["a1"]);
    menu_card(s);
}

/// Now Playing with the playing song starred.
fn fav_np(s: &mut State) {
    now_playing(s);
    s.star_for_scene(&["s-2-1"]);
}

// ---- scrolling player-bar lines ----

/// The playing song's title and "artist — album" are too long for the bar: they rest at the
/// start, cut with a soft edge (the slide itself only shows in a real window).
fn bar_long_title(s: &mut State) {
    album(s);
    if let Some(song) = &mut s.playback.song {
        song.title = "Requiem for the Last Night Bus Before the Depot Closes (Extended Choir Version)".into();
        song.artist = "The Midnight Timetable Orchestra & Friends".into();
    }
}

fn bar_long_title_floating(s: &mut State) {
    bar_long_title(s);
    s.settings.player_style = crate::settings::PlayerStyle::Floating;
}

// ---- local only mode: Songs, Artists, Albums, Folders ----

use crate::local::LocalTrack;

/// The scene cover as a file (a local track's thumbnail is a path).
fn scene_cover_path() -> std::path::PathBuf {
    let path = std::env::temp_dir().join("uzumaki-scene-cover.png");
    if !path.exists() {
        return save_temp_png("uzumaki-scene-cover.png", &uzumaki_fx::builtin(600, 600));
    }
    path
}

#[allow(clippy::too_many_arguments)]
fn ltrack(path: std::path::PathBuf, title: &str, artist: &str, album: &str, album_artist: &str, disc: Option<u32>, track: Option<u32>, year: Option<&str>, cover: bool) -> LocalTrack {
    LocalTrack {
        path,
        mtime: 1,
        size: 1,
        title: title.into(),
        artist: artist.into(),
        album: album.into(),
        album_fallback: album_artist == "Unknown Artist",
        album_artist: album_artist.into(),
        track,
        disc,
        year: year.map(String::from),
        genre: Some("Electronic".into()),
        duration: 150.0 + ((title.len() * 37) % 160) as f64,
        cover: cover.then(scene_cover_path),
    }
}

/// A small music folder: tagged albums by four artists (one with two discs), a compilation with
/// guests, and untagged rips (named after their folders, by Unknown Artist), some loose.
fn local_index() -> Vec<LocalTrack> {
    let m = home_dir().join("Music");
    let mut t = Vec::new();
    let albums: [(&str, &str, &str, &[&str], bool); 7] = [
        ("Aurora Lane", "Low Tide", "2019", &["Harbour Lights", "Low Tide", "Salt and Glass", "The Long Way Home", "Undertow", "Glasshouse"], true),
        ("Aurora Lane", "Paper Moons", "2022", &["Paper Moons", "Night Bus", "Fireflies", "Lantern", "Small Hours"], true),
        ("Kite Theory", "Cartography", "2017", &["North", "Contour Lines", "Legend", "Scale 1:50000", "True North", "Datum", "Hachures"], false),
        ("Mara Okafor", "Kilns", "2021", &["Clay", "Firing", "Glaze", "Shards"], true),
        ("Mara Okafor", "Early Recordings", "2012", &["Demo One", "Demo Two"], false),
        ("Nightjar", "Dusk Chorus", "2020", &["Roost", "Churr", "Moth Flight", "Heath at Ten"], true),
        ("Velvet Static", "Signal Loss", "2016", &["Carrier", "Dropout", "Snow", "Test Card", "Close Down"], false),
    ];
    for (artist, album, year, songs, cover) in albums {
        for (i, title) in songs.iter().enumerate() {
            let p = m.join(artist).join(album).join(format!("{:02} {title}.flac", i + 1));
            t.push(ltrack(p, title, artist, album, "", Some(1), Some(i as u32 + 1), Some(year), cover));
        }
    }
    // Disc two of Cartography.
    for (i, title) in ["Isobar", "Relief", "Bearing"].iter().enumerate() {
        let p = m.join("Kite Theory").join("Cartography").join("CD2").join(format!("{:02} {title}.flac", i + 1));
        t.push(ltrack(p, title, "Kite Theory", "Cartography", "", Some(2), Some(i as u32 + 1), Some("2017"), false));
    }
    // A compilation: guests keep their own artist pages.
    for (i, (title, artist)) in [("Tidewater (Edit)", "Aurora Lane"), ("Grid Ref", "Kite Theory"), ("Feather", "Nightjar"), ("Fog Bank", "Hollow Pines")].iter().enumerate() {
        let p = m.join("Compilations").join("Late Night Radio").join(format!("{:02} {title}.mp3", i + 1));
        t.push(ltrack(p, title, artist, "Late Night Radio", "Various Artists", None, Some(i as u32 + 1), Some("2023"), true));
    }
    // Untagged rips: the scanner already filled in what it could, as for a real file.
    let unsorted = m.join("Unsorted");
    for (dir, files) in [("tape rip 1987", &["side a", "side b"][..]), ("CD1", &["01 track", "02 track", "03 track"][..])] {
        for f in files {
            t.push(ltrack(unsorted.join(dir).join(format!("{f}.mp3")), f, "Unknown Artist", dir, "Unknown Artist", None, None, None, false));
        }
    }
    for f in ["voice memo 3", "untitled loop"] {
        t.push(ltrack(unsorted.join(format!("{f}.wav")), f, "Unknown Artist", "Unsorted", "Unknown Artist", None, None, None, false));
    }
    t
}

/// Local only mode over `tracks`, on `route` (navigated to, so a local page gets its songs).
fn local_only_with(s: &mut State, tracks: &[LocalTrack], route: Route) {
    base(s);
    let music = home_dir().join("Music");
    s.settings.music_folders = vec![music];
    s.set_music_source(MusicSource::LocalOnly);
    s.session.bridge_up = false;
    s.session.authorized = false;
    let (songs, view) = crate::local::view::from_index(tracks, &s.settings.music_folders);
    s.set_local_index(songs, view);
    s.navigate(route);
}

fn local_album_id(s: &State, title: &str) -> String {
    s.local_lib.view.albums.iter().find(|c| c.title == title).map(|c| c.id.clone()).unwrap_or_default()
}

fn local_only_songs(s: &mut State) {
    local_only_with(s, &local_index(), Route::LocalFiles);
}

fn local_only_albums(s: &mut State) {
    local_only_with(s, &local_index(), Route::LocalAlbums);
}

fn local_only_artists(s: &mut State) {
    local_only_with(s, &local_index(), Route::LocalArtists);
}

/// Aurora Lane: two albums, then their songs, the compilation's guest song last.
fn local_only_artist(s: &mut State) {
    local_only_with(s, &local_index(), Route::LocalFiles);
    let a = crate::local::view::artist_id("Aurora Lane");
    s.navigate(Route::Artist { id: a, name: "Aurora Lane".into() });
}

/// Cartography: two discs, no cover (the initials tile).
fn local_only_album(s: &mut State) {
    local_only_with(s, &local_index(), Route::LocalAlbums);
    let id = local_album_id(s, "Cartography");
    s.navigate(Route::Collection { kind: CollectionKind::Album, id });
}

/// An untagged folder as an album: its folder's name, Unknown Artist, the file names.
fn local_only_album_untagged(s: &mut State) {
    local_only_with(s, &local_index(), Route::LocalAlbums);
    let id = local_album_id(s, "tape rip 1987");
    s.navigate(Route::Collection { kind: CollectionKind::Album, id });
}

fn local_only_folders(s: &mut State) {
    local_only_with(s, &local_index(), Route::Folders { path: None });
}

/// Music: its folders, nothing loose.
fn local_only_folder(s: &mut State) {
    local_only_with(s, &local_index(), Route::Folders { path: Some(home_dir().join("Music")) });
}

/// Music/Unsorted: two untagged folders, then two loose files.
fn local_only_folder_untagged(s: &mut State) {
    local_only_with(s, &local_index(), Route::Folders { path: Some(home_dir().join("Music/Unsorted")) });
}

/// 20,000 songs (12 per album, 8 albums per artist, every 10th album an untagged folder), for
/// frame times on a large library.
fn big_index() -> Vec<LocalTrack> {
    let m = home_dir().join("Music");
    (0..20_000)
        .map(|i| {
            let (album, artist) = (i / 12, i / 96);
            let n = (i % 12 + 1) as u32;
            if album % 10 == 9 {
                let f = format!("track {n:02}");
                ltrack(m.join("Unsorted").join(format!("rip {album}")).join(format!("{f}.mp3")), &f, "Unknown Artist", &format!("rip {album}"), "Unknown Artist", None, None, None, false)
            } else {
                let (ar, al) = (format!("Artist {artist:04}"), format!("Album {album:05}"));
                let title = format!("Song {i}");
                ltrack(m.join(&ar).join(&al).join(format!("{n:02} {title}.flac")), &title, &ar, &al, "", Some(1), Some(n), Some(&format!("{}", 1970 + album % 50)), album % 3 == 0)
            }
        })
        .collect()
}

fn local_only_big_songs(s: &mut State) {
    local_only_with(s, &big_index(), Route::LocalFiles);
}

fn local_only_big_albums(s: &mut State) {
    local_only_with(s, &big_index(), Route::LocalAlbums);
}

fn local_only_big_artists(s: &mut State) {
    local_only_with(s, &big_index(), Route::LocalArtists);
}

fn local_only_big_artist(s: &mut State) {
    local_only_with(s, &big_index(), Route::LocalFiles);
    let a = crate::local::view::artist_id("Artist 0042");
    s.navigate(Route::Artist { id: a, name: "Artist 0042".into() });
}

fn local_only_big_folder(s: &mut State) {
    local_only_with(s, &big_index(), Route::Folders { path: Some(home_dir().join("Music/Unsorted")) });
}
