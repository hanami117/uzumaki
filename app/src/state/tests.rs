use super::*;
use crate::models::{Artwork, Collection};
use crate::cache::{Cached, Stored};

fn album(id: &str) -> Collection {
    Collection {
        kind: CollectionKind::Album,
        id: id.into(),
        title: format!("Album {id}"),
        subtitle: "Artist".into(),
        artwork: None,
        year: None,
        genre: None,
        description: None,
        track_count: None,
        modified: None,
        editable: false,
        favorites: false,
        public: None,
        tracks: Vec::new(),
        artists: Vec::new(),
    }
}

#[test]
fn a_late_answer_to_an_older_search_is_dropped() {
    let mut s = State::offline();
    s.search("first");
    s.search("second");
    let stale = Msg::Search { seq: 1, result: Ok(SearchResults::default()) };
    assert!(!s.apply_msg(stale));
    assert_eq!(s.search, Load::Idle);
    let fresh = Msg::Search { seq: 2, result: Err("boom".into()) };
    assert!(s.apply_msg(fresh));
    assert_eq!(s.search, Load::Failed("boom".into()));
    assert_eq!(s.route, Route::Search);
    assert_eq!(s.search_query, "second");
}

#[test]
fn library_pages_append_until_there_is_no_next() {
    let mut s = State::offline();
    let page = |ids: &[&str], next: Option<&str>| Page { items: ids.iter().map(|i| album(i)).collect(), next: next.map(String::from), total: None };
    s.apply_msg(Msg::LibraryAlbums(Ok(page(&["1", "2"], Some("/v1/me/library/albums?offset=2")))));
    assert_eq!(s.library_albums.load, Load::Loading);
    s.apply_msg(Msg::LibraryAlbums(Ok(page(&["3"], None))));
    assert_eq!(s.library_albums.load, Load::Ready(()));
    let ids: Vec<_> = s.library_albums.items.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(ids, ["1", "2", "3"]);
}

#[test]
fn signing_in_clears_personal_lists_and_hides_the_login_window() {
    let mut s = State::offline();
    s.session.login_window = true;
    s.library_albums.items.push(album("old"));
    s.library_albums.load = Load::Ready(());
    s.apply_msg(Msg::Bridge(Event::Auth { authorized: true, storefront: None }));
    assert!(s.session.authorized);
    assert!(!s.session.login_window);
    assert!(s.library_albums.items.is_empty());
    assert_eq!(s.library_albums.load, Load::Idle);
}

/// `--login` while the cache already says signed in: the page's `ready` hides the window too,
/// and the personal lists aren't thrown away (nothing changed).
#[test]
fn an_already_signed_in_ready_hides_the_login_window() {
    let mut s = State::offline();
    s.session.authorized = true;
    s.session.login_window = true;
    s.library_albums.items.push(album("kept"));
    s.library_albums.load = Load::Ready(());
    s.apply_msg(Msg::Bridge(Event::Ready { authorized: true, storefront: Some("it".into()), volume: None }));
    assert!(!s.session.login_window);
    assert_eq!(s.library_albums.items.len(), 1);
}

#[test]
fn time_ticks_only_redraw_when_the_second_changes() {
    let mut s = State::offline();
    assert!(s.apply_msg(Msg::Bridge(Event::Time { current: 1.2, duration: 200.0 })));
    assert!(!s.apply_msg(Msg::Bridge(Event::Time { current: 1.7, duration: 200.0 })));
    assert!(s.apply_msg(Msg::Bridge(Event::Time { current: 2.1, duration: 200.0 })));
}

#[test]
fn navigation_keeps_a_back_stack() {
    let mut s = State::offline();
    s.navigate(Route::Library(LibraryTab::Albums));
    s.open_collection(&album("42"));
    assert_eq!(s.route, Route::Collection { kind: CollectionKind::Album, id: "42".into() });
    s.back();
    assert_eq!(s.route, Route::Library(LibraryTab::Albums));
    s.back();
    assert_eq!(s.route, Route::Home);
    s.back();
    assert_eq!(s.route, Route::Home);
}

fn song(id: &str) -> Song {
    Song {
        id: id.into(),
        catalog_id: None,
        title: format!("Song {id}"),
        artist: "A".into(),
        album: "B".into(),
        duration: 200.0,
        track_number: None,
        disc_number: None,
        explicit: false,
        artwork: None,
        playable: true,
        genre: None,
        year: None,
        album_id: None,
        artists: Vec::new(),
    }
}

#[test]
fn clicking_a_track_shows_it_loading_at_once_and_a_double_click_is_one_request() {
    let mut s = State::offline();
    let songs = vec![song("1"), song("2"), song("3")];
    s.play_songs(&songs, 1);
    assert_eq!(s.playback.song.as_ref().map(|x| x.id.as_str()), Some("2"));
    assert_eq!(s.playback.state, PlayState::Loading);
    // The bridge's nowPlaying for another song lands, then the second click of the double-click:
    s.apply_msg(Msg::Bridge(Event::NowPlaying(Some(song("9")))));
    s.play_songs(&songs, 1);
    assert_eq!(s.playback.song.as_ref().map(|x| x.id.as_str()), Some("9"), "second click ignored");
    // A different row is a new request.
    s.play_songs(&songs, 2);
    assert_eq!(s.playback.song.as_ref().map(|x| x.id.as_str()), Some("3"));
}

#[test]
fn the_shuffle_button_starts_at_a_random_song_and_a_clicked_row_still_starts_there() {
    let mut s = State::offline();
    let mut c = album("a.1");
    c.tracks = (0..30).map(|i| song(&i.to_string())).collect();
    c.tracks[5].playable = false;
    super::queue::seed_for_tests(7);
    let mut firsts = std::collections::BTreeSet::new();
    for _ in 0..40 {
        s.last_play = None;
        s.playback.state = PlayState::Stopped; // as if the last one had started
        s.shuffle_collection(&c);
        assert!(s.playback.shuffle, "the button turns shuffle on");
        let first = s.playback.song.clone().unwrap();
        assert!(first.playable && first.id != "5");
        assert_eq!(s.queue.current().map(|x| &x.id), Some(&first.id), "the queue opens with it");
        let mut all: Vec<_> = s.queue.context.iter().map(|x| x.id.clone()).collect();
        all.sort();
        all.dedup();
        assert_eq!(all.len(), 30, "every song once");
        firsts.insert(first.id);
    }
    assert!(firsts.len() > 3, "not always the first song: {firsts:?}");
    // The same for any list (Local Files, library songs). The last song has started: a pick of
    // that same song mustn't be taken for a second click on a loading row.
    s.last_play = None;
    s.playback.state = PlayState::Stopped;
    s.shuffle_play(&c.tracks, "Local Files");
    assert_eq!(s.queue.source, "Local Files");
    // A clicked row with shuffle on starts at that row.
    s.last_play = None;
    s.playback.state = PlayState::Stopped;
    s.play_list(&c.tracks, 7, &c.title);
    assert_eq!(s.playback.song.as_ref().map(|x| x.id.as_str()), Some("7"));
    assert_eq!(s.queue.current().map(|x| x.id.as_str()), Some("7"));
}

#[test]
fn switching_songs_does_not_flicker_the_player_bar() {
    let mut s = State::offline();
    s.apply_msg(Msg::Bridge(Event::NowPlaying(Some(song("1")))));
    s.apply_msg(Msg::Bridge(Event::PlayState(PlayState::Playing)));
    let songs = vec![song("1"), song("2")];
    s.play_songs(&songs, 1);
    // The old queue winds down: MusicKit reports nothing playing and paused.
    assert!(!s.apply_msg(Msg::Bridge(Event::NowPlaying(None))));
    assert!(!s.apply_msg(Msg::Bridge(Event::PlayState(PlayState::Paused))));
    assert_eq!(s.playback.song.as_ref().map(|x| x.id.as_str()), Some("2"));
    assert_eq!(s.playback.state, PlayState::Loading);
    // The new song starts; our copy (with its artwork) is kept.
    let mut confirmed = song("2");
    confirmed.artwork = Some(crate::models::Artwork { template: "https://other/600x600bb.jpg".into(), bg: None });
    s.apply_msg(Msg::Bridge(Event::NowPlaying(Some(confirmed))));
    s.apply_msg(Msg::Bridge(Event::PlayState(PlayState::Playing)));
    assert_eq!(s.playback.state, PlayState::Playing);
    assert!(s.playback.song.as_ref().unwrap().artwork.is_none(), "kept the optimistic copy");
    // After it plays, a real pause shows.
    assert!(s.apply_msg(Msg::Bridge(Event::PlayState(PlayState::Paused))));
}

// ------------------------------------------------------------------ disk cache

fn cached_albums(ids: &[&str]) -> Cached {
    Cached { authorized: true, albums: Some(ids.iter().map(|i| album(i)).collect()), ..Default::default() }
}

fn album_page(ids: &[&str], next: Option<&str>) -> Page<Collection> {
    Page { items: ids.iter().map(|i| album(i)).collect(), next: next.map(String::from), total: None }
}

/// A state showing the Albums tab with the bridge "up", after `cached` was applied.
fn cached_state(cached: Cached) -> State {
    let mut s = State::offline();
    s.apply_cached(cached);
    s.session.bridge_up = true;
    s.route = Route::Library(LibraryTab::Albums);
    s.load_route_data();
    s
}

fn sent_matching(s: &State, needle: &str) -> usize {
    s.sent.borrow().iter().filter(|p| p.contains(needle)).count()
}

fn ids(s: &State) -> Vec<&str> {
    s.library_albums.items.iter().map(|c| c.id.as_str()).collect()
}

#[test]
fn cached_list_shows_at_once_and_the_refresh_is_swapped_in_in_one_step() {
    let mut s = cached_state(cached_albums(&["1", "2"]));
    assert!(s.session.authorized, "the cache remembered we were signed in");
    assert_eq!(s.library_albums.load, Load::Ready(()));
    assert_eq!(ids(&s), ["1", "2"]);
    // The probe says the library changed: refresh behind the cached list.
    s.apply_msg(Msg::LibraryNewest { songs: false, result: Ok((Some(3), vec!["9".into()])) });
    assert!(s.library_albums.refreshing);
    assert_eq!(s.library_albums.load, Load::Ready(()), "still looks ready");
    s.apply_msg(Msg::LibraryAlbums(Ok(album_page(&["9", "1"], Some("/v1/me/library/albums?offset=2")))));
    assert_eq!(ids(&s), ["1", "2"], "cached items stay until the last page");
    assert_eq!(s.library_albums.load, Load::Ready(()));
    s.apply_msg(Msg::LibraryAlbums(Ok(album_page(&["2"], None))));
    assert_eq!(ids(&s), ["9", "1", "2"]);
    assert!(!s.library_albums.refreshing);
}

#[test]
fn failed_refresh_keeps_the_cached_list() {
    let mut s = cached_state(cached_albums(&["1"]));
    s.apply_msg(Msg::LibraryNewest { songs: false, result: Ok((Some(2), vec!["9".into()])) });
    s.apply_msg(Msg::LibraryAlbums(Err("offline".into())));
    assert_eq!(ids(&s), ["1"]);
    assert_eq!(s.library_albums.load, Load::Ready(()));
}

#[test]
fn matching_probe_means_no_full_fetch() {
    let mut s = cached_state(cached_albums(&["3", "2", "1"]));
    assert_eq!(sent_matching(&s, "/library/albums"), 1, "just the probe");
    assert!(sent_matching(&s, "\"limit\":10") >= 1);
    s.apply_msg(Msg::LibraryNewest { songs: false, result: Ok((Some(3), vec!["3".into(), "2".into(), "1".into()])) });
    assert!(!s.library_albums.refreshing);
    assert_eq!(sent_matching(&s, "/library/albums"), 1, "no full fetch");
    // Revisiting the tab doesn't probe again this session.
    s.load_route_data();
    assert_eq!(sent_matching(&s, "/library/albums"), 1);
    // A different total does refresh.
    let mut s = cached_state(cached_albums(&["3", "2", "1"]));
    s.apply_msg(Msg::LibraryNewest { songs: false, result: Ok((Some(2), vec!["3".into(), "2".into()])) });
    assert!(s.library_albums.refreshing);
    assert_eq!(sent_matching(&s, "/library/albums"), 2);
}

fn playlist(id: &str, modified: &str) -> Collection {
    let mut c = album(id);
    c.kind = CollectionKind::Playlist;
    c.modified = Some(modified.into());
    c.tracks = vec![song("1")];
    c
}

/// Cached playlist list + cached detail page of `p.1`, with the playlist route open.
fn cached_playlist_state(detail_modified: &str) -> State {
    let mut s = State::offline();
    let mut listed = playlist("p.1", detail_modified);
    listed.tracks.clear();
    s.apply_cached(Cached { authorized: true, playlists: Some(vec![listed]), ..Default::default() });
    s.session.bridge_up = true;
    s.route = Route::Collection { kind: CollectionKind::Playlist, id: "p.1".into() };
    let stored = Stored { saved_at: 0, data: playlist("p.1", detail_modified) };
    s.apply_cached_collection("p.1".into(), Some(stored));
    s
}

#[test]
fn unchanged_playlist_uses_the_cache_without_a_request() {
    let mut s = cached_playlist_state("v1");
    assert!(s.collections["p.1"].ready().is_some(), "shown from the cache");
    assert_eq!(sent_matching(&s, "/library/playlists/p.1"), 0, "waits for the list to be re-fetched");
    assert_eq!(sent_matching(&s, "/library/playlists"), 1, "the list is always re-fetched");
    let mut fresh = playlist("p.1", "v1");
    fresh.tracks.clear();
    s.apply_msg(Msg::LibraryPlaylists(Ok(Page { items: vec![fresh], next: None, total: None })));
    assert_eq!(sent_matching(&s, "/library/playlists/p.1"), 0);
    assert!(s.collections["p.1"].ready().is_some());
}

fn signed(date: &str) -> Artwork {
    // What Apple gives for a picture someone uploaded: a signed link, no {w}x{h}, good for a day.
    Artwork {
        template: format!("https://store-033.blobstore.apple.com/x/image?X-Amz-Date={date}&X-Amz-Expires=86400&X-Amz-Signature=ab"),
        bg: None,
    }
}

/// A picture someone uploaded to a playlist is a link that dies after a day. The page saved
/// yesterday has the dead one (Apple answers 406 "wrong time info"), the list just re-fetched has
/// a live one: the page must show the live one (measured on the user's cache, 2026-11-10).
#[test]
fn a_cached_playlist_page_takes_the_lists_fresh_picture_link() {
    let mut s = State::offline();
    let mut listed = playlist("p.1", "v1");
    listed.tracks.clear();
    listed.artwork = Some(signed("20261110T180120Z"));
    s.apply_cached(Cached { authorized: true, playlists: Some(vec![listed.clone()]), ..Default::default() });
    s.session.bridge_up = true;
    s.route = Route::Collection { kind: CollectionKind::Playlist, id: "p.1".into() };
    let mut old = playlist("p.1", "v1");
    old.artwork = Some(signed("20261109T094431Z"));
    s.apply_cached_collection("p.1".into(), Some(Stored { saved_at: 0, data: old }));
    s.apply_msg(Msg::LibraryPlaylists(Ok(Page { items: vec![listed], next: None, total: None })));
    let c = s.collections["p.1"].ready().unwrap();
    assert_eq!(c.artwork, Some(signed("20261110T180120Z")));
    assert_eq!(s.cover_of(c).unwrap().url(1200), signed("20261110T180120Z").template, "no size to substitute");
    assert_eq!(sent_matching(&s, "/library/playlists/p.1"), 0, "still no page fetch: only the link changed");
}

#[test]
fn a_pinned_playlist_uses_the_lists_picture_link() {
    let mut s = State::offline();
    let mut listed = playlist("p.1", "v1");
    listed.artwork = Some(signed("20261110T180120Z"));
    s.library_playlists.items = vec![listed];
    let pin = crate::settings::Pin {
        kind: crate::settings::PinKind::Playlist,
        id: "p.1".into(),
        title: "X".into(),
        artwork: Some(signed("20261109T094431Z")),
    };
    assert_eq!(s.pin_cover(&pin), Some(&signed("20261110T180120Z")));
    s.library_playlists.items.clear();
    assert_eq!(s.pin_cover(&pin), Some(&signed("20261109T094431Z")), "no listing: the saved one");
}

#[test]
fn modified_playlist_is_refetched_behind_the_cached_copy() {
    let mut s = cached_playlist_state("v1");
    let mut fresh = playlist("p.1", "v2");
    fresh.tracks.clear();
    s.apply_msg(Msg::LibraryPlaylists(Ok(Page { items: vec![fresh], next: None, total: None })));
    assert_eq!(sent_matching(&s, "/library/playlists/p.1"), 1);
    assert_eq!(s.collections["p.1"].ready().unwrap().modified.as_deref(), Some("v1"), "cached copy still shown");
    let mut new = playlist("p.1", "v2");
    new.tracks = vec![song("1"), song("2")];
    s.apply_msg(Msg::Collection { id: "p.1".into(), result: Ok((new, Some("/next".into()))) });
    assert_eq!(s.collections["p.1"].ready().unwrap().tracks.len(), 1, "swap waits for the last page");
    s.apply_msg(Msg::CollectionTracks { id: "p.1".into(), result: Ok(Page { items: vec![song("3")], next: None, total: None }) });
    let c = s.collections["p.1"].ready().unwrap();
    assert_eq!((c.modified.as_deref(), c.tracks.len()), (Some("v2"), 3));
}

/// Album `id` with one artist, as Apple sends it now.
fn album_with_artist(id: &str) -> Collection {
    Collection { artists: vec![Artist { id: "7".into(), name: "Artist".into() }], ..album(id) }
}

/// The album page of `100`, shown from a cache file saved at `saved_at`.
fn open_cached_album(saved_at: u64, data: Collection) -> State {
    let mut s = State::offline();
    s.session.bridge_up = true;
    s.route = Route::Collection { kind: CollectionKind::Album, id: "100".into() };
    s.apply_cached_collection("100".into(), Some(Stored { saved_at, data }));
    s
}

#[test]
fn catalog_pages_are_reused_for_a_day() {
    let open = |saved_at: u64| open_cached_album(saved_at, album_with_artist("100"));
    assert_eq!(sent_matching(&open(crate::cache::now_secs() - 3600), "/albums/100"), 0);
    let old = open(crate::cache::now_secs() - 2 * 24 * 3600);
    assert_eq!(sent_matching(&old, "/albums/100"), 1);
    assert!(old.collections["100"].ready().is_some(), "old copy shown while it refreshes");
}

#[test]
fn an_album_cached_without_artists_is_asked_for_once_behind_the_cached_copy() {
    // Saved an hour ago by a build that didn't keep artist ids: the name couldn't link.
    let mut s = open_cached_album(crate::cache::now_secs() - 3600, album("100"));
    assert_eq!(sent_matching(&s, "/albums/100"), 1);
    assert!(s.collections["100"].ready().is_some(), "the cached copy stays shown meanwhile");
    s.apply_msg(Msg::Collection { id: "100".into(), result: Ok((album_with_artist("100"), None)) });
    assert_eq!(s.collections["100"].ready().unwrap().artists.len(), 1, "the fresh copy has the ids");
    // Going away and back this session asks nothing more, even if Apple had sent no artists.
    s.navigate(Route::Home);
    s.navigate(Route::Collection { kind: CollectionKind::Album, id: "100".into() });
    assert_eq!(sent_matching(&s, "/albums/100"), 1);
    // A cached copy with the ids but no names (a catalog album asked without include=artists).
    let nameless = Collection { artists: vec![Artist { id: "7".into(), name: String::new() }], ..album("100") };
    assert_eq!(sent_matching(&open_cached_album(crate::cache::now_secs() - 3600, nameless), "/albums/100"), 1);
}

#[test]
fn signing_out_drops_the_cached_library() {
    let mut s = cached_state(cached_albums(&["1"]));
    assert_eq!(ids(&s), ["1"]);
    s.apply_msg(Msg::Bridge(Event::Ready { authorized: false, storefront: None, volume: None }));
    assert!(!s.session.authorized);
    assert!(s.library_albums.items.is_empty());
    assert_eq!(s.library_albums.load, Load::Idle);
}

#[test]
fn cache_is_not_shown_to_a_signed_out_bridge() {
    let mut s = State::offline();
    s.session.bridge_up = true; // the bridge already said: not signed in
    s.apply_cached(cached_albums(&["1"]));
    assert!(s.library_albums.items.is_empty());
}

#[test]
fn new_pins_go_on_top_and_can_be_dragged_anywhere() {
    let mut s = State::offline();
    for id in ["a", "b", "c"] {
        s.pin_collection(&album(id));
    }
    let order = |s: &State| s.settings.pins.iter().map(|p| p.id.clone()).collect::<Vec<_>>();
    assert_eq!(order(&s), ["c", "b", "a"]);
    s.move_pin_to("c", 2);
    assert_eq!(order(&s), ["b", "a", "c"]);
    s.move_pin_to("a", 0);
    assert_eq!(order(&s), ["a", "b", "c"]);
}

#[test]
fn late_time_events_never_move_the_playing_position_backwards() {
    crate::clock::freeze(); // the test moves time itself: a slow machine can't stretch it
    let mut s = State::offline();
    s.playback.state = PlayState::Playing;
    s.apply_msg(Msg::Bridge(Event::Time { current: 10.0, duration: 200.0 }));
    crate::clock::advance(std::time::Duration::from_millis(120));
    let before = s.playback.position_now();
    // A late event reports a position a bit behind what we've been showing.
    s.apply_msg(Msg::Bridge(Event::Time { current: 10.05, duration: 200.0 }));
    assert!(s.playback.position_now() >= before, "no step back on jitter");
    // A real seek backwards still jumps.
    s.apply_msg(Msg::Bridge(Event::Time { current: 3.0, duration: 200.0 }));
    assert!((s.playback.position - 3.0).abs() < 1e-9);
}

#[test]
fn whole_second_time_reports_never_move_the_clock_backwards() {
    crate::clock::freeze(); // the test moves time itself: a slow machine can't stretch it
    // MusicKit's own time is whole seconds: a report of n means somewhere in [n, n + 1).
    let mut s = State::offline();
    s.playback.state = PlayState::Playing;
    s.apply_msg(Msg::Bridge(Event::Time { current: 20.0, duration: 200.0 }));
    let mut last = s.playback.position_now();
    for i in 1..=12 {
        crate::clock::advance(std::time::Duration::from_millis(100));
        let now = s.playback.position_now();
        assert!(now >= last, "stepped back before an event: {now} < {last}");
        // Reports lag the true time (20 + 0.1 i) down to the whole second.
        s.apply_msg(Msg::Bridge(Event::Time { current: (20.0 + 0.1 * i as f64).floor(), duration: 200.0 }));
        let after = s.playback.position_now();
        assert!(after >= now - 1e-6, "stepped back on a report: {after} < {now}");
        last = after;
    }
    assert!((last - 21.2).abs() < 0.2, "clock drifted from the true time: {last}");
}

#[test]
fn a_late_report_slews_the_clock_instead_of_jumping() {
    crate::clock::freeze(); // the test moves time itself: a slow machine can't stretch it
    let mut s = State::offline();
    s.playback.state = PlayState::Playing;
    s.apply_msg(Msg::Bridge(Event::Time { current: 10.25, duration: 200.0 }));
    crate::clock::advance(std::time::Duration::from_millis(100));
    let before = s.playback.position_now();
    // 0.4 s behind what we show: keep going, slower, until it is worked off.
    s.apply_msg(Msg::Bridge(Event::Time { current: before - 0.4, duration: 200.0 }));
    assert!(s.playback.position_now() >= before);
    assert!((s.playback.slew - 0.4).abs() < 0.01);
}

#[test]
fn reports_from_before_a_seek_do_not_snap_the_clock_back() {
    let mut s = State::offline();
    s.playback.state = PlayState::Playing;
    s.apply_msg(Msg::Bridge(Event::Time { current: 140.0, duration: 200.0 }));
    s.seek_seconds(60.4);
    // MusicKit's old whole-second time, still on its way: ignored.
    assert!(!s.apply_msg(Msg::Bridge(Event::Time { current: 141.0, duration: 200.0 })));
    let t = s.playback.position_now();
    assert!((60.4..61.0).contains(&t), "snapped back to {t}");
    // The player confirms the seek: followed, and the guard is done.
    s.apply_msg(Msg::Bridge(Event::Time { current: 60.5, duration: 200.0 }));
    assert!(s.playback.seek_target.is_none());
    // Later jumps are seeks again (another app, media keys on the page).
    s.apply_msg(Msg::Bridge(Event::Time { current: 10.0, duration: 200.0 }));
    assert!((s.playback.position - 10.0).abs() < 1e-9);
}

#[test]
fn a_seek_the_player_never_confirms_gives_way() {
    let mut s = State::offline();
    s.playback.state = PlayState::Playing;
    s.playback.duration = 200.0;
    s.seek_seconds(60.0);
    let long_ago = std::time::Instant::now() - std::time::Duration::from_secs(4);
    s.playback.seek_target = Some((60.0, long_ago));
    s.apply_msg(Msg::Bridge(Event::Time { current: 141.0, duration: 200.0 }));
    assert!((s.playback.position - 141.0).abs() < 1e-9);
    assert!(s.playback.seek_target.is_none());
}

#[test]
fn repeat_one_wrapping_after_a_seek_to_the_end_is_followed() {
    let mut s = State::offline();
    s.playback.state = PlayState::Playing;
    s.playback.duration = 200.0;
    s.seek_seconds(199.5);
    s.apply_msg(Msg::Bridge(Event::Time { current: 0.2, duration: 200.0 }));
    assert!(s.playback.seek_target.is_none());
    assert!(s.playback.position_now() < 1.0);
}

#[test]
fn a_new_song_ends_the_seek_guard() {
    let mut s = State::offline();
    s.playback.state = PlayState::Playing;
    s.playback.duration = 200.0;
    s.seek_seconds(150.0);
    s.apply_msg(Msg::Bridge(Event::NowPlaying(Some(song("next")))));
    // Far from the old target (150 s), and believed: it is the new song's time.
    s.apply_msg(Msg::Bridge(Event::Time { current: 5.0, duration: 180.0 }));
    assert!((s.playback.position - 5.0).abs() < 1e-9);
}

#[test]
fn resuming_after_a_pause_does_not_count_the_paused_time() {
    crate::clock::freeze(); // the test moves time itself: a slow machine can't stretch it
    let mut s = State::offline();
    s.apply_msg(Msg::Bridge(Event::PlayState(PlayState::Playing)));
    s.apply_msg(Msg::Bridge(Event::Time { current: 30.5, duration: 200.0 }));
    s.apply_msg(Msg::Bridge(Event::PlayState(PlayState::Paused)));
    let paused_at = s.playback.position_now();
    crate::clock::advance(std::time::Duration::from_millis(150));
    s.apply_msg(Msg::Bridge(Event::PlayState(PlayState::Playing)));
    assert!(s.playback.position_now() - paused_at < 0.05, "resume jumped ahead by the pause");
}

#[test]
fn signing_out_drops_the_library_queue_and_player() {
    let mut s = State::offline();
    s.session.bridge_up = true;
    s.session.authorized = true;
    s.library_albums = Paged::ready(vec![album("l.1")]);
    s.collections.insert("p.1".into(), Load::Ready(album("p.1")));
    s.collections.insert("1440".into(), Load::Ready(album("1440")));
    s.play_songs(&[song("a"), song("b")], 0);
    s.playback.volume = 0.4;
    s.sign_out(); // headless: no bridge to wait for
    assert!(!s.session.authorized && !s.session.signing_out);
    assert!(s.library_albums.items.is_empty());
    assert!(!s.collections.contains_key("p.1"), "personal pages go");
    assert!(s.collections.contains_key("1440"), "catalog pages stay");
    assert!(s.playback.song.is_none() && s.queue.current().is_none());
    assert_eq!(s.playback.volume, 0.4, "volume is MusicKit's; keep it");
    assert!(s.toast.is_some());
}

#[test]
fn a_failed_sign_out_keeps_the_session() {
    let mut s = State::offline();
    s.session.authorized = true;
    s.session.signing_out = true;
    s.apply_msg(Msg::SignedOut(Err("boom".into())));
    assert!(s.session.authorized && !s.session.signing_out);
    assert!(s.toast.as_deref().is_some_and(|t| t.contains("boom")));
}

#[test]
fn the_last_page_is_remembered_only_when_asked() {
    use crate::settings::StartPage;
    let mut s = State::offline();
    s.navigate(Route::Radio);
    assert_eq!(s.settings.last_route, None);
    s.set_start_page(StartPage::Last);
    assert_eq!(s.settings.last_route, Some(Route::Radio), "the page showing now");
    s.navigate(Route::Library(LibraryTab::Songs));
    assert_eq!(s.settings.last_route, Some(Route::Library(LibraryTab::Songs)));
    s.back();
    assert_eq!(s.settings.last_route, Some(Route::Radio));
    s.set_start_page(StartPage::Home);
    assert_eq!(s.settings.last_route, None);
}

#[test]
fn shuffle_repeat_and_data_saver_are_settings() {
    let mut s = State::offline();
    s.toggle_shuffle();
    s.cycle_repeat();
    assert!(s.settings.shuffle);
    assert_eq!(s.settings.repeat, Repeat::All);
    s.set_data_saver(true);
    assert!(s.settings.data_saver);
    s.set_tray_icon(false);
    s.set_close_to_tray(true);
    assert!(!s.settings.closes_to_tray(), "close to tray needs the tray icon");
    assert!(!s.hide_on_close(), "no tray in tests: closing quits");
}

// ---------------------------------------------------------------- local files (batch D)

fn local(name: &str) -> Song {
    Song { id: format!("file:///music/{name}.flac"), title: name.into(), ..song(name) }
}

fn local_sent(s: &State) -> Vec<String> {
    s.local_sent.borrow().clone()
}

/// The local player reports `event` about the song it was last given.
fn from_local(s: &mut State, event: Event) -> bool {
    let generation = s.local_gen;
    s.apply_msg(Msg::Local { generation, event })
}

#[test]
fn a_local_file_plays_in_process_and_the_paused_bridge_is_ignored() {
    let mut s = State::offline();
    s.play_list(&[local("x"), local("y")], 0, "Local Files");
    assert!(s.playback.local);
    assert_eq!(local_sent(&s), ["play x"]);
    assert_eq!(s.playback.state, PlayState::Loading);
    assert!(from_local(&mut s, Event::PlayState(PlayState::Playing)));
    // The bridge pausing (we paused it) or ticking must not touch what the bar shows.
    assert!(!s.apply_msg(Msg::Bridge(Event::PlayState(PlayState::Paused))));
    assert!(!s.apply_msg(Msg::Bridge(Event::NowPlaying(Some(song("a"))))));
    assert_eq!(s.playback.state, PlayState::Playing);
    assert_eq!(s.playback.song.as_ref().unwrap().title, "x");
    s.toggle_play();
    s.seek_to(0.5);
    assert_eq!(local_sent(&s), ["play x", "pause", "seek 100"]);
}

#[test]
fn after_a_local_file_the_app_starts_the_next_song_and_drops_stale_events() {
    let mut s = State::offline();
    s.play_list(&[local("x"), local("y"), local("z")], 0, "L");
    let old = s.local_gen;
    from_local(&mut s, Event::PlayState(PlayState::Ended));
    assert_eq!(s.playback.song.as_ref().unwrap().title, "y");
    assert_eq!(local_sent(&s), ["play x", "play y"]);
    // A late event about x (older generation) must not skip y.
    assert!(!s.apply_msg(Msg::Local { generation: old, event: Event::PlayState(PlayState::Ended) }));
    assert_eq!(s.playback.song.as_ref().unwrap().title, "y");
    s.next();
    assert_eq!(s.playback.song.as_ref().unwrap().title, "z");
    s.playback.position = 1.0;
    s.previous();
    assert_eq!(s.playback.song.as_ref().unwrap().title, "y");
}

#[test]
fn the_end_of_a_local_list_stops_and_play_starts_it_again() {
    let mut s = State::offline();
    s.play_list(&[local("x")], 0, "L");
    from_local(&mut s, Event::PlayState(PlayState::Playing));
    from_local(&mut s, Event::PlayState(PlayState::Ended));
    assert_eq!(s.playback.state, PlayState::Ended);
    assert!(s.playback.local, "the local player stays in charge");
    s.toggle_play();
    assert_eq!(local_sent(&s), ["play x", "play x"]);
}

#[test]
fn repeat_one_replays_a_local_file() {
    let mut s = State::offline();
    s.playback.repeat = Repeat::One;
    s.play_list(&[local("x"), local("y")], 0, "L");
    from_local(&mut s, Event::PlayState(PlayState::Ended));
    assert_eq!(s.playback.song.as_ref().unwrap().title, "x");
    assert_eq!(local_sent(&s), ["play x", "play x"]);
}

#[test]
fn a_local_file_hands_over_to_the_bridge_for_an_apple_song() {
    let mut s = State::offline();
    s.play_list(&[local("x"), song("a"), song("b")], 0, "L");
    from_local(&mut s, Event::PlayState(PlayState::Ended));
    assert!(!s.playback.local);
    assert_eq!(local_sent(&s), ["play x", "stop"]);
    assert_eq!(s.playback.song.as_ref().unwrap().id, "a");
    assert_eq!(s.queue.upcoming_ids(), ["b"]);
    // Now the bridge drives again.
    assert!(s.apply_msg(Msg::Bridge(Event::NowPlaying(Some(song("a"))))));
    assert!(s.apply_msg(Msg::Bridge(Event::PlayState(PlayState::Playing))));
    assert_eq!(s.playback.state, PlayState::Playing);
}

/// MusicKit plays `a` to near its end and reports `end`.
fn apple_song_runs_out(s: &mut State, end: PlayState) {
    s.apply_msg(Msg::Bridge(Event::NowPlaying(Some(song("a")))));
    s.apply_msg(Msg::Bridge(Event::PlayState(PlayState::Playing)));
    s.apply_msg(Msg::Bridge(Event::Time { current: 198.0, duration: 200.0 }));
    s.apply_msg(Msg::Bridge(Event::PlayState(end)));
}

#[test]
fn when_musickits_queue_runs_out_before_a_local_file_the_app_plays_the_file() {
    for end in [PlayState::Ended, PlayState::Completed, PlayState::Stopped] {
        let mut s = State::offline();
        s.play_list(&[song("a"), local("x"), song("b")], 0, "L");
        assert!(s.queue.upcoming_ids().is_empty(), "MusicKit only gets a");
        apple_song_runs_out(&mut s, end);
        assert!(s.playback.local, "{end:?}");
        assert_eq!(local_sent(&s), ["play x"]);
        // A late "ended" from the bridge changes nothing now.
        assert!(!s.apply_msg(Msg::Bridge(Event::PlayState(PlayState::Ended))));
    }
}

#[test]
fn an_apple_song_ending_while_musickit_has_the_next_one_is_left_to_musickit() {
    let mut s = State::offline();
    s.play_list(&[song("a"), song("b"), local("x")], 0, "L");
    apple_song_runs_out(&mut s, PlayState::Ended);
    assert!(!s.playback.local);
    assert!(local_sent(&s).is_empty());
    // MusicKit moved on to b first, then the old song's "stopped" arrives: not the end of b.
    s.apply_msg(Msg::Bridge(Event::NowPlaying(Some(song("b")))));
    s.apply_msg(Msg::Bridge(Event::PlayState(PlayState::Stopped)));
    assert!(!s.playback.local, "b just started; only its real end hands over to x");
}

#[test]
fn next_on_an_apple_song_before_a_local_file_starts_the_file() {
    let mut s = State::offline();
    s.play_list(&[song("a"), local("x")], 0, "L");
    s.apply_msg(Msg::Bridge(Event::NowPlaying(Some(song("a")))));
    s.next();
    assert!(s.playback.local);
    assert_eq!(local_sent(&s), ["play x"]);
    // Previous at the start of the file goes back to the Apple song (a fresh bridge queue).
    s.playback.position = 0.5;
    s.previous();
    assert!(!s.playback.local);
    assert_eq!(s.playback.song.as_ref().unwrap().id, "a");
}

#[test]
fn repeat_all_over_a_mixed_list_is_wrapped_by_the_app() {
    let mut s = State::offline();
    s.playback.repeat = Repeat::All;
    s.play_list(&[local("x"), song("a")], 0, "L");
    from_local(&mut s, Event::PlayState(PlayState::Ended));
    assert_eq!(s.playback.song.as_ref().unwrap().id, "a");
    apple_song_runs_out(&mut s, PlayState::Completed);
    assert!(s.playback.local, "wrapped to x");
    assert_eq!(local_sent(&s), ["play x", "stop", "play x"]);
}

#[test]
fn a_play_next_local_file_plays_after_the_current_apple_song() {
    let mut s = State::offline();
    s.play_list(&[song("a"), song("b")], 0, "L");
    s.apply_msg(Msg::Bridge(Event::NowPlaying(Some(song("a")))));
    s.play_next(&[local("x")]);
    assert!(s.queue.upcoming_ids().is_empty(), "MusicKit stops after a");
    apple_song_runs_out(&mut s, PlayState::Ended);
    assert_eq!(local_sent(&s), ["play x"]);
    from_local(&mut s, Event::PlayState(PlayState::Ended));
    assert_eq!(s.playback.song.as_ref().unwrap().id, "b");
    assert!(!s.playback.local);
}

#[test]
fn local_songs_never_ask_for_lyrics_or_go_to_apple_playlists() {
    let mut s = State::offline();
    s.session.authorized = true;
    s.play_list(&[local("x")], 0, "L");
    assert!(s.lyrics.is_empty());
    s.go_to_album(&local("x"));
    assert_eq!(s.route, Route::LocalFiles);
}

#[test]
fn removing_a_music_folder_drops_its_songs_at_once() {
    let mut s = State::offline();
    s.settings.music_folders = vec!["/music".into(), "/other".into()];
    s.local_lib.songs = vec![local("x"), Song { id: "file:///other/y.flac".into(), ..local("y") }];
    s.remove_music_folder(0);
    assert_eq!(s.settings.music_folders, [std::path::PathBuf::from("/other")]);
    assert_eq!(s.local_lib.songs.len(), 1);
    s.add_music_folder("/other/sub".into());
    assert_eq!(s.settings.music_folders.len(), 1, "already inside a music folder");
    s.add_music_folder("/".into());
    assert_eq!(s.settings.music_folders, [std::path::PathBuf::from("/")], "a parent replaces its children");
}

// ---------------------------------------------------------------- batch E

#[test]
fn a_new_playlist_request_names_it_and_carries_only_apple_songs() {
    use super::playlists::new_playlist_body;
    let mut lib = song("i.lib");
    lib.catalog_id = None;
    let cat = Song { catalog_id: Some("123".into()), ..song("i.x") };
    let local = Song { id: "file:///m/a.flac".into(), ..song("a") };
    let body = new_playlist_body(" Road trip ", "  ", &[lib, cat, local]);
    assert_eq!(body["attributes"]["name"], " Road trip ", "the caller trims the name");
    assert!(body["attributes"].get("description").is_none(), "blank description left out");
    let refs = body["relationships"]["tracks"]["data"].as_array().unwrap();
    assert_eq!(refs.len(), 2);
    assert_eq!(refs[0], serde_json::json!({"id": "i.lib", "type": "library-songs"}));
    assert_eq!(refs[1], serde_json::json!({"id": "123", "type": "songs"}));
    let empty = new_playlist_body("Empty", "For later", &[]);
    assert_eq!(empty["attributes"]["description"], "For later");
    assert!(empty.get("relationships").is_none());
}

#[test]
fn creating_a_playlist_needs_a_name_and_a_signed_in_account() {
    let mut s = State::offline();
    s.create_playlist("Mix", "", &[]);
    assert!(s.toast.as_deref().unwrap().contains("Sign in"));
    s.session.authorized = true;
    s.create_playlist("   ", "", &[]);
    assert!(s.sent.borrow().is_empty(), "no name, no request");
    s.create_playlist(" Mix ", "", &[song("1")]);
    let sent = s.sent.borrow().clone();
    assert_eq!(sent.len(), 1);
    assert!(sent[0].starts_with("POST /v1/me/library/playlists") && sent[0].contains("\"Mix\""), "{sent:?}");
}

#[test]
fn a_created_playlist_is_listed_first_and_opened() {
    let mut s = State::offline();
    s.library_playlists = Paged::ready(vec![Collection { kind: CollectionKind::Playlist, ..album("p.old") }]);
    s.creating_playlist = true;
    let made = Collection { kind: CollectionKind::Playlist, title: String::new(), ..album("p.new") };
    s.apply_msg(Msg::PlaylistCreated { name: "Mix".into(), songs: vec![song("1")], result: Ok(made) });
    assert!(!s.creating_playlist);
    assert_eq!(s.library_playlists.items[0].id, "p.new");
    assert_eq!(s.library_playlists.items[0].title, "Mix", "Apple's echo may lack the name");
    assert!(s.library_playlists.items[0].editable);
    assert_eq!(s.route, Route::Collection { kind: CollectionKind::Playlist, id: "p.new".into() });
    assert_eq!(s.collections["p.new"].ready().unwrap().tracks.len(), 1, "its page shows the songs at once");
    assert_eq!(s.toast.as_deref(), Some("Created “Mix”"));
    s.creating_playlist = true;
    s.apply_msg(Msg::PlaylistCreated { name: "Bad".into(), songs: vec![], result: Err("HTTP 403".into()) });
    assert!(!s.creating_playlist);
    assert_eq!(s.toast.as_deref(), Some("Couldn't create “Bad”: HTTP 403"));
}

#[test]
fn the_right_sidebar_buttons_switch_tabs_or_close_it() {
    use crate::settings::RightTab;
    let mut s = State::offline();
    s.toggle_right_panel();
    assert!(s.settings.right_panel);
    s.show_right_tab(RightTab::Queue);
    assert!(s.settings.right_panel && s.settings.right_tab == RightTab::Queue, "switches tab, stays open");
    s.show_right_tab(RightTab::Queue);
    assert!(!s.settings.right_panel, "the same tab again closes it");
    s.show_right_tab(RightTab::Lyrics);
    assert!(s.settings.right_panel && s.settings.right_tab == RightTab::Lyrics);
}

#[test]
fn now_playing_lyrics_and_up_next_take_turns_in_the_column() {
    let mut s = State::offline();
    assert!(s.settings.now_playing_lyrics && !s.settings.now_playing_queue);
    s.toggle_now_playing_queue();
    assert!(s.settings.now_playing_queue, "Up Next takes the column");
    s.toggle_now_playing_lyrics();
    assert!(s.settings.now_playing_lyrics && !s.settings.now_playing_queue, "the lyrics button switches back");
    s.toggle_now_playing_lyrics();
    assert!(!s.settings.now_playing_lyrics, "and then hides the lyrics, as before");
    s.toggle_now_playing_queue();
    s.toggle_now_playing_queue();
    assert!(!s.settings.now_playing_queue && !s.settings.now_playing_lyrics, "closing Up Next leaves the column empty again");
}

#[test]
fn fine_time_stays_on_while_either_lyrics_view_needs_it() {
    let mut s = State::offline();
    s.set_panel_fine_time(true);
    s.set_fine_time(false); // Now Playing closed: the sidebar still shows lyrics
    assert!(s.fine_time);
    s.set_panel_fine_time(false);
    assert!(!s.fine_time);
}

#[test]
fn keyboard_steps_clamp_volume_and_position() {
    let mut s = State::offline();
    s.playback.volume = 0.98;
    s.volume_by(0.05);
    assert_eq!(s.playback.volume, 1.0);
    s.playback.song = Some(song("1"));
    s.playback.duration = 200.0;
    s.playback.position = 5.0;
    s.seek_by(-10.0);
    assert_eq!(s.playback.position, 0.0);
    s.seek_by(500.0);
    assert_eq!(s.playback.position, 200.0);
}

#[test]
fn late_volume_echoes_dont_pull_the_volume_back_while_it_changes() {
    let mut s = State::offline();
    s.set_volume(0.5);
    s.set_volume(0.6);
    assert!(!s.apply_msg(Msg::Bridge(Event::Volume(0.5))), "echo of the earlier step");
    assert_eq!(s.playback.volume, 0.6);
    s.volume_set_at = Some(Instant::now() - VOLUME_ECHO * 2);
    assert!(s.apply_msg(Msg::Bridge(Event::Volume(0.3))), "later, a change from the page counts");
    assert_eq!(s.playback.volume, 0.3);
}

#[test]
fn the_songs_table_view_and_sorts_are_remembered() {
    // An older settings file without `table`: playlists as Songs, newest additions first.
    let st: crate::settings::Settings = serde_json::from_str("{}").unwrap();
    assert!(st.table.songs_view);
    assert_eq!(st.table.playlist.col, None);
    assert!(!st.table.playlist.asc);
    let mut s = State::offline();
    s.ui.table.songs_view = false;
    s.ui.table.click(crate::ui::Which::Library, Some(crate::ui::Col::Artist));
    s.remember_table();
    assert!(!s.settings.table.songs_view);
    assert_eq!(s.settings.table.library.col, Some(crate::ui::Col::Artist));
    // Next launch: restored from settings.
    let mut fresh = crate::ui::UiState::default();
    fresh.table.apply(s.settings.table);
    assert!(!fresh.table.songs_view);
    assert_eq!(fresh.table.lib_sort.col, Some(crate::ui::Col::Artist));
}

#[test]
fn adding_catalog_songs_and_albums_to_the_library() {
    let mut s = State::offline();
    s.session.authorized = true;
    let cat = Song { catalog_id: Some("111".into()), ..song("i.lib") }; // already in the library
    s.add_songs_to_library(&[song("222"), song("333"), cat, song("222")]);
    assert_eq!(s.sent.borrow().last().unwrap(), "POST /v1/me/library?ids[songs]=111,222,333");
    s.add_collection_to_library(&Collection { kind: CollectionKind::Album, ..album("1440857781") });
    assert_eq!(s.sent.borrow().last().unwrap(), "POST /v1/me/library?ids[albums]=1440857781");
    let before = s.sent.borrow().len();
    s.add_collection_to_library(&album("l.mine"));
    assert_eq!(s.sent.borrow().len(), before, "a library album is already there");
    assert!(s.toast.as_deref().unwrap().contains("already in your library"));
    s.library_songs = Paged::ready(vec![song("i.1")]);
    s.apply_msg(Msg::AddedToLibrary { what: "“X”".into(), result: Ok(()) });
    assert_eq!(s.toast.as_deref(), Some("Added “X” to your library"));
    assert!(s.library_songs.stale, "the library is checked again on the next visit");
}

/// A playlist A, B, A on its page.
fn aba(s: &mut State) -> Collection {
    let pl = Collection {
        kind: CollectionKind::Playlist,
        editable: true,
        track_count: Some(3),
        tracks: vec![song("i.a"), song("i.b"), song("i.a")],
        title: "Mix".into(),
        ..album("p.mix")
    };
    s.collections.insert("p.mix".into(), Load::Ready(pl.clone()));
    pl
}

fn page_ids(s: &State) -> Vec<String> {
    s.collections.get("p.mix").and_then(|l| l.ready()).map(|c| c.tracks.iter().map(|t| t.id.clone()).collect()).unwrap_or_default()
}

fn entries(ids: &[(&str, &str)], next: Option<&str>) -> Msg {
    let items = ids.iter().map(|(i, k)| (i.to_string(), k.to_string())).collect();
    Msg::PlaylistEntries { id: "p.mix".into(), result: Ok(Page { items, next: next.map(String::from), total: None }) }
}

const ENTRIES: &str = r#"/v1/me/library/playlists/p.mix/tracks {"limit":100,"omit[resource]":"autos"}"#;

#[test]
fn removing_every_copy_of_a_song_from_a_playlist_is_one_delete() {
    let mut s = State::offline();
    let pl = aba(&mut s);
    s.remove_from_playlist(&pl, &[(0, song("i.a")), (2, song("i.a"))]);
    assert_eq!(s.sent.borrow().as_slice(), ["DELETE /v1/me/library/playlists/p.mix/tracks?ids[library-songs]=i.a&mode=all"]);
    assert_eq!(page_ids(&s), ["i.b"]);
    assert_eq!(s.collections["p.mix"].ready().unwrap().track_count, Some(1), "both rows counted off");
    // A song listed once still goes with its one row (now at position 0).
    s.remove_from_playlist(&pl, &[(0, song("i.b"))]);
    assert_eq!(s.sent.borrow().last().unwrap(), "DELETE /v1/me/library/playlists/p.mix/tracks?ids[library-songs]=i.b&mode=all");
    s.apply_msg(Msg::RemovedFromPlaylist { id: "p.mix".into(), name: "Mix".into(), count: 1, result: Err("HTTP 404".into()) });
    assert_eq!(s.toast.as_deref(), Some("Couldn't remove from “Mix”: HTTP 404"));
    assert!(!s.collections.get("p.mix").is_some_and(|l| l.ready().is_some()), "dropped so Apple's copy loads again");
    // Someone else's playlist can't be edited.
    let theirs = Collection { kind: CollectionKind::Playlist, editable: false, ..album("pl.apple") };
    let before = s.sent.borrow().len();
    s.remove_from_playlist(&theirs, &[(0, song("i.b"))]);
    assert_eq!(s.sent.borrow().len(), before);
}

#[test]
fn removing_one_copy_of_a_song_rewrites_the_playlist_without_that_row() {
    let mut s = State::offline();
    let pl = aba(&mut s);
    // The second "i.a": Apple's list is read first, the page updates at once.
    s.remove_from_playlist(&pl, &[(2, song("i.a"))]);
    assert_eq!(s.sent.borrow().as_slice(), [ENTRIES]);
    assert_eq!(page_ids(&s), ["i.a", "i.b"]);
    assert_eq!(s.collections["p.mix"].ready().unwrap().track_count, Some(2));
    // A second edit waits for this one.
    s.remove_from_playlist(&pl, &[(1, song("i.b"))]);
    assert_eq!(s.sent.borrow().len(), 1);
    assert!(s.toast.as_deref().unwrap().contains("Still saving"), "{:?}", s.toast);
    // Apple's list matches what was shown: written back without position 2, types as Apple gave them.
    s.apply_msg(entries(&[("i.a", "library-songs"), ("i.b", "library-music-videos"), ("i.a", "library-songs")], None));
    assert_eq!(
        s.sent.borrow().last().unwrap(),
        r#"PUT /v1/me/library/playlists/p.mix/tracks {"data":[{"id":"i.a","type":"library-songs"},{"id":"i.b","type":"library-music-videos"}]}"#
    );
    assert!(!s.sent.borrow().iter().any(|r| r.contains("mode=")), "never mode=first, never mode=all here");
    s.apply_msg(Msg::RemovedFromPlaylist { id: "p.mix".into(), name: "Mix".into(), count: 1, result: Ok(()) });
    assert_eq!(s.toast.as_deref(), Some("Removed from “Mix”"));
    assert_eq!(page_ids(&s), ["i.a", "i.b"]);
}

#[test]
fn removing_the_first_copy_drops_position_zero() {
    let mut s = State::offline();
    let pl = aba(&mut s);
    s.remove_from_playlist(&pl, &[(0, song("i.a"))]);
    assert_eq!(page_ids(&s), ["i.b", "i.a"]);
    s.apply_msg(entries(&[("i.a", "library-songs"), ("i.b", "library-songs"), ("i.a", "library-songs")], None));
    assert_eq!(
        s.sent.borrow().last().unwrap(),
        r#"PUT /v1/me/library/playlists/p.mix/tracks {"data":[{"id":"i.b","type":"library-songs"},{"id":"i.a","type":"library-songs"}]}"#
    );
}

#[test]
fn a_playlist_changed_on_apple_is_reloaded_and_nothing_is_written() {
    for apple in [
        vec![("i.a", "library-songs"), ("i.a", "library-songs"), ("i.b", "library-songs")], // reordered
        vec![("i.a", "library-songs"), ("i.b", "library-songs")],                           // shorter
        vec![("i.a", "library-songs"), ("i.b", "library-songs"), ("i.a", "library-songs"), ("i.c", "library-songs")], // longer
    ] {
        let mut s = State::offline();
        let pl = aba(&mut s);
        s.remove_from_playlist(&pl, &[(2, song("i.a"))]);
        s.apply_msg(entries(&apple, None));
        assert!(!s.sent.borrow().iter().any(|r| r.starts_with("PUT") || r.starts_with("DELETE")), "{:?}", s.sent.borrow());
        assert!(s.toast.as_deref().unwrap().contains("changed on Apple Music"), "{:?}", s.toast);
        assert!(!s.collections.get("p.mix").is_some_and(|l| l.ready().is_some()), "reloaded from Apple");
    }
    // Apple refusing the read reloads too.
    let mut s = State::offline();
    let pl = aba(&mut s);
    s.remove_from_playlist(&pl, &[(2, song("i.a"))]);
    s.apply_msg(Msg::PlaylistEntries { id: "p.mix".into(), result: Err("HTTP 500".into()) });
    assert_eq!(s.toast.as_deref(), Some("Couldn't remove from “Mix”: HTTP 500"));
    assert!(!s.sent.borrow().iter().any(|r| r.starts_with("PUT")));
    assert!(!s.collections.get("p.mix").is_some_and(|l| l.ready().is_some()));
}

#[test]
fn a_rewrite_reads_every_page_of_apples_list() {
    let mut s = State::offline();
    let pl = aba(&mut s);
    s.remove_from_playlist(&pl, &[(2, song("i.a"))]);
    s.apply_msg(entries(&[("i.a", "library-songs"), ("i.b", "library-songs")], Some("/v1/me/library/playlists/p.mix/tracks?offset=2")));
    assert_eq!(
        s.sent.borrow().last().unwrap(),
        r#"/v1/me/library/playlists/p.mix/tracks?offset=2 {"limit":100,"omit[resource]":"autos"}"#
    );
    assert!(!s.sent.borrow().iter().any(|r| r.starts_with("PUT")), "not before the last page");
    s.apply_msg(entries(&[("i.a", "library-songs")], None));
    assert_eq!(
        s.sent.borrow().last().unwrap(),
        r#"PUT /v1/me/library/playlists/p.mix/tracks {"data":[{"id":"i.a","type":"library-songs"},{"id":"i.b","type":"library-songs"}]}"#
    );
}

#[test]
fn a_row_no_longer_where_it_was_removes_nothing() {
    let mut s = State::offline();
    let pl = aba(&mut s);
    s.remove_from_playlist(&pl, &[(1, song("i.a"))]);
    assert!(s.sent.borrow().is_empty());
    assert_eq!(page_ids(&s), ["i.a", "i.b", "i.a"]);
}

#[test]
fn removing_songs_albums_and_playlists_from_the_library() {
    let mut s = State::offline();
    s.session.authorized = true;
    s.library_songs = Paged::ready(vec![song("i.a"), song("i.b")]);
    s.collections.insert("p.x".into(), Load::Ready(Collection { kind: CollectionKind::Playlist, tracks: vec![song("i.a"), song("i.b")], ..album("p.x") }));
    s.remove_songs_from_library(&[song("i.a"), song("123")]);
    assert_eq!(s.sent.borrow().last().unwrap(), "DELETE /v1/me/library/songs/i.a", "catalog songs aren't in the library");
    s.apply_msg(Msg::RemovedFromLibrary { kind: "songs", id: "i.a".into(), title: "A".into(), result: Ok(()) });
    assert_eq!(s.library_songs.items.len(), 1);
    assert_eq!(s.collections["p.x"].ready().unwrap().tracks.len(), 1, "gone from the playlists on screen too");
    assert_eq!(s.toast.as_deref(), Some("Removed “A” from your library"));

    // Deleting the open playlist: unpinned, unlisted, and the page goes back.
    let pl = Collection { kind: CollectionKind::Playlist, editable: true, ..album("p.x") };
    s.library_playlists = Paged::ready(vec![pl.clone()]);
    s.settings.pins.push(crate::settings::Pin { kind: crate::settings::PinKind::Playlist, id: "p.x".into(), title: "X".into(), artwork: None });
    s.navigate(Route::Collection { kind: CollectionKind::Playlist, id: "p.x".into() });
    s.remove_collection_from_library(&pl);
    assert_eq!(s.sent.borrow().last().unwrap(), "DELETE /v1/me/library/playlists/p.x");
    s.apply_msg(Msg::RemovedFromLibrary { kind: "playlists", id: "p.x".into(), title: "X".into(), result: Ok(()) });
    assert!(s.library_playlists.items.is_empty() && s.settings.pins.is_empty());
    assert_ne!(s.route, Route::Collection { kind: CollectionKind::Playlist, id: "p.x".into() });

    s.remove_collection_from_library(&album("l.alb"));
    assert_eq!(s.sent.borrow().last().unwrap(), "DELETE /v1/me/library/albums/l.alb");
    let before = s.sent.borrow().len();
    s.remove_collection_from_library(&album("1440857781"));
    assert_eq!(s.sent.borrow().len(), before, "a catalog album isn't in the library");
    s.apply_msg(Msg::RemovedFromLibrary { kind: "albums", id: "l.alb".into(), title: "Alb".into(), result: Err("HTTP 403".into()) });
    assert_eq!(s.toast.as_deref(), Some("Couldn't remove “Alb”: HTTP 403"));
}

// ------------------------------------------------------------------------ uzu (command line)

mod uzu {
    use super::*;
    use uzumaki_ctl as ctl;

    fn ids(songs: &[ctl::SongInfo]) -> Vec<&str> {
        songs.iter().map(|s| s.id.as_str()).collect()
    }

    #[test]
    fn status_shows_the_song_and_its_place_in_the_list() {
        let mut s = State::offline();
        let st = s.ctl_status();
        assert_eq!((st.state, st.song.is_none(), st.index, st.count), (ctl::PlayState::Stopped, true, None, 0));
        s.play_list(&[song("1"), song("2"), song("3")], 1, "Mix");
        let st = s.ctl_status();
        assert_eq!(st.song.as_ref().map(|x| x.id.as_str()), Some("2"));
        assert_eq!((st.state, st.index, st.count, st.source.as_str()), (ctl::PlayState::Loading, Some(1), 3, "Mix"));
        s.set_shuffle(true);
        s.play_list(&[song("1"), song("2"), song("3")], 2, "Mix");
        assert_eq!(s.ctl_status().index, Some(2), "the list's own number, not the shuffled order");
    }

    #[test]
    fn a_queued_song_playing_has_no_list_index() {
        let mut s = State::offline();
        s.play_list(&[song("1"), song("2")], 0, "Mix");
        s.add_to_queue(&[song("9")]);
        assert_eq!(s.ctl_status().queued, 1);
        let q = s.ctl_queue();
        assert_eq!((ids(&q.up_next), q.queued), (vec!["9", "2"], 1));
        s.apply_msg(Msg::Bridge(Event::NowPlaying(Some(song("9")))));
        let st = s.ctl_status();
        assert_eq!((st.song.map(|x| x.id), st.index, st.queued), (Some("9".to_string()), None, 0));
    }

    #[test]
    fn playlists_list_favourites_first_with_known_counts() {
        let mut s = State::offline();
        let mut fav = playlist("p.fav", "v1");
        fav.favorites = true;
        fav.tracks.clear();
        let mut mine = playlist("p.mine", "v1");
        mine.tracks.clear();
        s.library_playlists.items = vec![mine, fav];
        let mut loaded = playlist("p.mine", "v1");
        loaded.tracks = vec![song("1"), song("2")];
        s.collections.insert("p.mine".into(), Load::Ready(loaded));
        let list = s.ctl_playlist_entries();
        assert_eq!(list.iter().map(|(c, _)| c.id.as_str()).collect::<Vec<_>>(), ["p.fav", "p.mine"]);
        assert_eq!((list[0].0.count, list[1].0.count), (None, Some(2)));
        assert_eq!(list[1].1.as_deref(), Some("v1"), "with the listed modified date");
    }

    #[test]
    fn pinned_playlists_come_first_in_pin_order() {
        let mut s = State::offline();
        let mut fav = playlist("p.fav", "v1");
        fav.favorites = true;
        s.library_playlists.items = vec![playlist("p.a", "v1"), fav, playlist("p.b", "v1"), playlist("p.c", "v1")];
        // Sidebar pins, top to bottom (newest on top): p.c, album l.x, p.a. Albums don't count.
        s.pin_collection(&playlist("p.a", "v1"));
        s.pin_collection(&album("l.x"));
        s.pin_collection(&playlist("p.c", "v1"));
        let list = s.ctl_playlist_entries();
        let order: Vec<_> = list.iter().map(|(c, _)| (c.id.as_str(), c.pinned)).collect();
        assert_eq!(order, [("p.c", Some(0)), ("p.a", Some(1)), ("p.fav", None), ("p.b", None)]);
        s.move_pin_to("p.a", 0);
        let ids: Vec<_> = s.ctl_playlist_entries().into_iter().map(|(c, _)| c.id).collect();
        assert_eq!(ids, ["p.a", "p.c", "p.fav", "p.b"], "follows the sidebar's order");
    }

    #[test]
    fn playing_a_collection_checks_the_song_number_and_availability() {
        let mut s = State::offline();
        let mut c = playlist("p.1", "v1");
        c.title = "Road".into();
        c.tracks = vec![song("1"), song("2")];
        c.tracks[1].playable = false;
        assert_eq!(s.ctl_play_collection(&c, Some(5), None), Err("“Road” has 2 songs".into()));
        assert_eq!(s.ctl_play_collection(&c, Some(1), None), Err("“Song 2” isn't available on Apple Music".into()));
        assert!(s.playback.song.is_none());
        assert_eq!(s.ctl_play_collection(&c, Some(0), None), Ok(()));
        assert_eq!((s.playback.song.as_ref().map(|x| x.id.as_str()), s.queue.source.as_str()), (Some("1"), "Road"));
    }

    #[test]
    fn an_ordered_start_plays_the_songs_in_the_clients_order() {
        let mut s = State::offline();
        let mut c = playlist("p.1", "v1");
        c.title = "Road".into();
        c.tracks = (0..5).map(|i| song(&i.to_string())).collect();
        c.tracks[3].playable = false;
        let up = |s: &State| s.queue.up_next().map(|x| x.id.clone()).collect::<Vec<_>>();
        // `index` is a position within `order`: here the song "4", then the rest as sorted.
        s.ctl_play_collection(&c, Some(1), Some(&[2, 4, 0, 3, 1])).unwrap();
        assert_eq!(s.playback.song.as_ref().map(|x| x.id.as_str()), Some("4"));
        assert_eq!(up(&s), ["0", "3", "1"]);
        assert_eq!(s.queue.source, "Road");
        // No index: from the first playable song of the order.
        s.last_play = None;
        s.ctl_play_collection(&c, None, Some(&[3, 1, 0, 2, 4])).unwrap();
        assert_eq!(s.playback.song.as_ref().map(|x| x.id.as_str()), Some("1"));
        assert_eq!(up(&s), ["0", "2", "4"]);
        // With shuffle: it still starts at the chosen song, the rest in some order.
        s.set_shuffle(true);
        s.last_play = None;
        s.ctl_play_collection(&c, Some(0), Some(&[4, 3, 2, 1, 0])).unwrap();
        assert_eq!(s.playback.song.as_ref().map(|x| x.id.as_str()), Some("4"));
        let mut rest = up(&s);
        rest.sort();
        assert_eq!(rest, ["0", "1", "2", "3"]);
        // The position is checked against the reordered list, and so are the indices.
        s.last_play = None;
        assert_eq!(s.ctl_play_collection(&c, Some(1), Some(&[0, 3, 2])), Err("“Song 3” isn't available on Apple Music".into()));
        assert_eq!(s.ctl_play_collection(&c, Some(3), Some(&[0, 1, 2])), Err("“Road” has 3 songs".into()));
        let stale = Err("this list changed since it was loaded; press R to reload".to_string());
        assert_eq!(s.ctl_play_collection(&c, Some(0), Some(&[0, 5])), stale);
        assert_eq!(s.ctl_play_collection(&c, None, Some(&[9])), stale);
    }

    #[test]
    fn no_song_number_means_the_first_playable_or_a_random_one_with_shuffle() {
        let mut s = State::offline();
        let mut c = playlist("p.1", "v1");
        c.tracks = (1..=50).map(|i| song(&i.to_string())).collect();
        c.tracks[0].playable = false;
        s.ctl_play_collection(&c, None, None).unwrap();
        assert_eq!(s.playback.song.as_ref().map(|x| x.id.as_str()), Some("2"));
        s.set_shuffle(true);
        crate::state::queue::seed_for_tests(11);
        let firsts: std::collections::HashSet<String> = (0..20)
            .map(|_| {
                s.last_play = None; // not a double-click
                s.playback.state = PlayState::Stopped; // nor a click on the song still loading
                s.ctl_play_collection(&c, None, None).unwrap();
                s.playback.song.clone().unwrap().id
            })
            .collect();
        assert!(firsts.len() > 3, "random starts: {firsts:?}");
        assert!(!firsts.contains("1"), "never the unplayable one");
        c.tracks.iter_mut().for_each(|t| t.playable = false);
        assert!(s.ctl_play_collection(&c, None, None).is_err());
    }

    #[test]
    fn adopted_collections_never_replace_loaded_ones() {
        let mut s = State::offline();
        s.ctl_adopt(playlist("p.1", "v1"), Some(5));
        assert!(matches!(s.coll_cache.get("p.1"), Some(CollCache::Cached(5))));
        let mut other = playlist("p.1", "v2");
        other.tracks = vec![song("7")];
        s.ctl_adopt(other, None);
        let Some(Load::Ready(c)) = s.collections.get("p.1") else { panic!("not loaded") };
        assert_eq!(c.modified.as_deref(), Some("v1"));
        assert_eq!(s.ctl_lookup("p.1").0.map(|c| c.id), Some("p.1".to_string()));
    }

    #[test]
    fn cached_copies_follow_the_detail_page_rules() {
        let stored = |id: &str, saved_at: u64| Stored { saved_at, data: playlist(id, "v1") };
        assert!(super::super::ctl::cached_is_current(&stored("p.1", 0), Some("v1")));
        assert!(!super::super::ctl::cached_is_current(&stored("p.1", 0), Some("v2")));
        assert!(!super::super::ctl::cached_is_current(&stored("p.1", 0), None));
        assert!(super::super::ctl::cached_is_current(&stored("pl.cat", cache::now_secs()), None));
        assert!(!super::super::ctl::cached_is_current(&stored("pl.cat", 0), None));
    }

    #[test]
    fn jump_seek_shuffle_and_repeat() {
        let mut s = State::offline();
        assert_eq!(s.ctl_jump(0), Err("Up Next is empty".into()));
        s.play_list(&[song("1"), song("2"), song("3")], 0, "Mix");
        assert_eq!(s.ctl_jump(5), Err("Up Next has 2 songs".into()));
        assert_eq!(s.ctl_jump(1), Ok(()));
        assert_eq!(s.playback.song.as_ref().map(|x| x.id.as_str()), Some("3"));
        s.ctl_seek(ctl::SeekTo::Percent(50.0));
        assert_eq!(s.playback.position, 100.0);
        s.ctl_seek(ctl::SeekTo::Relative(-150.0));
        assert_eq!(s.playback.position, 0.0);
        s.set_shuffle(true);
        s.set_shuffle(true);
        assert!(s.playback.shuffle && s.settings.shuffle);
        s.set_repeat(Repeat::One);
        assert_eq!((s.playback.repeat, s.settings.repeat), (Repeat::One, Repeat::One));
        s.ctl_volume(uzumaki_ctl::VolumeTo::Set(0.5));
        s.ctl_volume(uzumaki_ctl::VolumeTo::Change(0.8));
        assert_eq!(s.playback.volume, 1.0);
    }

    #[test]
    fn picked_search_results_skip_unplayable_songs() {
        let mut s = State::offline();
        let mut gone = song("x");
        gone.playable = false;
        assert!(s.ctl_pick(vec![gone.clone()], uzumaki_ctl::PickAction::Play, "Search").is_err());
        s.ctl_pick(vec![gone, song("1"), song("2")], uzumaki_ctl::PickAction::Play, "Search “a”").unwrap();
        assert_eq!((s.playback.song.as_ref().map(|x| x.id.as_str()), s.queue.source.as_str()), (Some("1"), "Search “a”"));
        s.ctl_pick(vec![song("3")], uzumaki_ctl::PickAction::Next, "").unwrap();
        assert_eq!(s.queue.up_next().map(|x| x.id.as_str()).collect::<Vec<_>>(), ["3", "2"]);
    }
}

// ------------------------------------------------------------------------ songs Apple can't find

fn library_song(id: &str, catalog: &str) -> Song {
    let mut s = song(id);
    s.catalog_id = Some(catalog.into());
    s
}

fn not_found(first: &str) -> Msg {
    Msg::QueueFailed { first: first.into(), error: format!("[mk-007] NOT_FOUND; One or more items could not be resolved: {first}") }
}

fn playing_id(s: &State) -> Option<&str> {
    s.playback.song.as_ref().map(|x| x.id.as_str())
}

#[test]
fn a_song_missing_from_the_catalog_is_tried_by_library_id_then_skipped() {
    let mut s = State::offline();
    s.play_list(&[library_song("i.1", "111"), library_song("i.2", "222")], 0, "Wave");
    s.apply_msg(not_found("111"));
    assert_eq!((playing_id(&s), s.toast.as_deref()), (Some("i.1"), None), "retried by library id, still loading");
    assert_eq!(s.playback.state, PlayState::Loading);
    s.apply_msg(not_found("i.1"));
    assert_eq!(playing_id(&s), Some("i.2"));
    assert_eq!(s.toast.as_deref(), Some("“Song i.1” isn't available on Apple Music, skipped"));
    assert!(!s.queue.original[0].playable, "not queued again");
    assert_eq!(s.queue.cycle_ids(), Some(vec!["222".to_string()]));
}

#[test]
fn a_failure_for_a_song_already_replaced_is_ignored() {
    let mut s = State::offline();
    s.play_list(&[song("1"), song("2")], 1, "");
    s.apply_msg(not_found("1"));
    assert_eq!((playing_id(&s), s.toast.as_deref()), (Some("2"), None));
}

#[test]
fn a_list_of_missing_songs_stops_after_a_few_skips() {
    let mut s = State::offline();
    let songs: Vec<Song> = (1..=8).map(|i| song(&i.to_string())).collect();
    s.play_list(&songs, 0, "");
    for i in 1..=MAX_SKIPS {
        s.apply_msg(not_found(&i.to_string()));
    }
    assert_eq!(playing_id(&s), Some("5"), "stays on the last one tried");
    assert_eq!(s.playback.state, PlayState::Stopped);
    assert_eq!(s.toast.as_deref(), Some("“Song 5” isn't available on Apple Music"));
}

#[test]
fn other_start_failures_stop_showing_loading() {
    let mut s = State::offline();
    s.play_list(&[song("1")], 0, "");
    s.apply_msg(Msg::QueueFailed { first: "1".into(), error: "play timed out".into() });
    assert_eq!(s.toast.as_deref(), Some("Couldn't play “Song 1”: play timed out"));
    assert!(s.starting.is_none(), "MusicKit's real state is shown again");
    assert_eq!(playing_id(&s), Some("1"));
}

#[test]
fn the_first_sync_saves_every_playlist_two_at_a_time() {
    let mut s = State::offline();
    s.session.bridge_up = true;
    s.session.authorized = true;
    s.syncing = true;
    s.library_songs.load = Load::Ready(());
    s.library_albums.load = Load::Ready(());
    let listed = || -> Vec<Collection> {
        ["p.1", "p.2", "p.3", "p.4"].iter().map(|id| Collection { tracks: Vec::new(), ..playlist(id, "v1") }).collect()
    };
    // p.4 is open already: its page loads and saves it.
    s.collections.insert("p.4".into(), Load::Loading);
    s.apply_msg(Msg::LibraryPlaylists(Ok(Page { items: listed(), next: None, total: None })));
    assert_eq!(sent_matching(&s, "/library/playlists/p."), 2, "two at a time");
    assert_eq!(s.sync_rest()[0], (0, 4));
    assert!(s.library_synced() && !s.sync_complete(), "the lists are in, the playlists aren't");

    s.apply_msg(Msg::SyncPlaylist { id: "p.1".into(), result: Ok((playlist("p.1", "v1"), Some("/p1-more".into()))) });
    assert_eq!(sent_matching(&s, "/p1-more"), 1, "the rest of its tracks");
    s.apply_msg(Msg::SyncPlaylist { id: "p.2".into(), result: Err("boom".into()) });
    assert_eq!(sent_matching(&s, "/library/playlists/p.3"), 1, "a failure moves on");
    assert_eq!(sent_matching(&s, "/library/playlists/p.4"), 0);
    s.apply_msg(Msg::SyncPlaylistTracks { id: "p.1".into(), result: Ok(Page { items: vec![song("2")], next: None, total: None }) });
    s.apply_msg(Msg::SyncPlaylist { id: "p.3".into(), result: Ok((playlist("p.3", "v1"), None)) });
    assert_eq!(s.sync_rest()[0], (4, 4), "p.4 counted as done");
    assert!(s.sync_complete());
    assert!(!s.collections.contains_key("p.1"), "saved to disk, not kept in memory");

    // The list again (an edit re-fetches it): nothing is fetched twice.
    let before = sent_matching(&s, "/library/playlists/p.");
    s.apply_msg(Msg::LibraryPlaylists(Ok(Page { items: listed(), next: None, total: None })));
    assert_eq!(sent_matching(&s, "/library/playlists/p."), before);

    // Signed out halfway: late answers are dropped.
    s.queue_playlist_sync(vec!["p.5".into()]);
    s.clear_personal();
    assert!(!s.apply_msg(Msg::SyncPlaylist { id: "p.5".into(), result: Ok((playlist("p.5", "v1"), None)) }));
}

#[test]
fn the_next_lap_is_decided_when_the_list_starts_and_the_bridge_loops_into_it() {
    // Every order: the hand-queued song 7 may also sit anywhere in the shuffled list, the last
    // place included (about 1 order in 19 — this test used to fail on those).
    for seed in 0..200 {
        super::queue::seed_for_tests(seed);
        next_lap_with_one_order(seed);
    }
}

fn next_lap_with_one_order(seed: u64) {
    let mut s = State::offline();
    s.set_repeat(Repeat::All);
    s.set_shuffle(true);
    let songs: Vec<Song> = (0..20).map(|i| library_song(&format!("i.{i}"), &format!("{}", 100 + i))).collect();
    s.play_list(&songs, 0, "L");
    assert!(s.queue.next_lap.is_some(), "planned at once, not on the last song");
    let shown = s.queue.next_lap_songs().to_vec();
    assert_eq!(shown.len(), 20);
    assert_eq!(s.queue.cycle_ids().unwrap(), shown.iter().map(|x| x.play_id().to_string()).collect::<Vec<_>>(), "the bridge loops into what Up Next shows");
    // An edit of the lap re-plans it; a Play Next doesn't change it.
    s.play_next(&[songs[7].clone()]);
    assert_eq!(s.queue.next_lap_songs(), &shown[..]);
    s.remove_up_next(1);
    assert_eq!(s.queue.next_lap_songs().len(), 19);
    // The wrap: MusicKit plays the hand-queued song, then the rest of the list, then reports
    // the first song of the shown lap; a new lap follows, and the bridge is told (lap_gen
    // moved, so `sync_upcoming` ran).
    let lap = s.queue.next_lap_songs().to_vec();
    s.apply_msg(Msg::Bridge(Event::NowPlaying(Some(songs[7].clone()))));
    let rest = s.queue.context[s.queue.pos + 1..].to_vec();
    for song in rest {
        s.apply_msg(Msg::Bridge(Event::NowPlaying(Some(song))));
    }
    assert_eq!(s.queue.pos + 1, s.queue.context.len(), "seed {seed}: on the lap's last song");
    s.apply_msg(Msg::Bridge(Event::NowPlaying(Some(lap[0].clone()))));
    assert_eq!(s.queue.context, lap, "seed {seed}: what was shown plays");
    assert_ne!(s.queue.next_lap_songs(), &lap[..]);
    s.set_repeat(Repeat::Off);
    assert!(s.queue.next_lap.is_none() && s.queue.next_lap_songs().is_empty(), "toggling repeat drops the plan");
}

#[test]
fn uzu_sees_the_next_lap_and_can_jump_into_it() {
    use uzumaki_ctl as ctl;
    let mut s = State::offline();
    s.set_repeat(Repeat::All);
    s.play_list(&[song("1"), song("2"), song("3")], 1, "Mix");
    let q = s.ctl_queue();
    assert_eq!(q.up_next.iter().map(|x| x.id.as_str()).collect::<Vec<_>>(), ["3"]);
    assert_eq!(q.next_lap.iter().map(|x| x.id.as_str()).collect::<Vec<_>>(), ["1", "2", "3"], "without shuffle the list order");
    assert_eq!(q.repeat, ctl::RepeatMode::All);
    assert_eq!(s.ctl_jump(5), Err("Up Next has 4 songs".into()));
    assert_eq!(s.ctl_jump(2), Ok(()), "entry 3 is the second song of the next lap");
    assert_eq!(s.playback.song.as_ref().map(|x| x.id.as_str()), Some("2"));
    assert_eq!(s.ctl_queue().up_next.iter().map(|x| x.id.as_str()).collect::<Vec<_>>(), ["3"]);
    s.set_repeat(Repeat::Off);
    assert!(s.ctl_queue().next_lap.is_empty());
    assert_eq!(s.ctl_jump(1), Err("Up Next has 1 songs".into()));
}

// ------------------------------------------------------------------------ stations

fn station() -> crate::models::Station {
    crate::models::Station { id: "ra.1".into(), name: "Stazione di Thomas".into(), artwork: None, live: false }
}

fn heard(s: &mut State, id: &str) {
    s.apply_msg(Msg::Bridge(Event::NowPlaying(Some(song(id)))));
}

fn played_ids(s: &State) -> Vec<&str> {
    s.queue.station_played().iter().map(|x| x.id.as_str()).collect()
}

#[test]
fn a_station_lists_the_songs_it_picked_and_uzu_sees_them() {
    let mut s = State::offline();
    s.play_list(&[song("old")], 0, "Mix"); // what played before is not part of the station
    s.play_station(&station());
    assert!(s.queue.station.is_some() && s.queue.up_next_len() == 0);
    for id in ["a", "b", "c"] {
        heard(&mut s, id);
    }
    assert_eq!(played_ids(&s), ["b", "a"]);
    s.add_to_queue(&[song("u")]);
    let q = s.ctl_queue();
    assert_eq!(q.station.as_deref(), Some("Stazione di Thomas"));
    assert_eq!(q.up_next.iter().map(|x| x.id.as_str()).collect::<Vec<_>>(), ["u"]);
    assert_eq!((q.queued, q.station_played.iter().map(|x| x.id.as_str()).collect::<Vec<_>>()), (1, vec!["b", "a"]));
    assert_eq!(s.ctl_jump(3), Err("Up Next has 3 songs".into()));
    // A normal list drops the station.
    s.play_list(&[song("x")], 0, "Mix");
    assert!(s.queue.station.is_none() && s.ctl_queue().station.is_none());
}

#[test]
fn a_played_song_replays_alone_then_the_station_picks_up() {
    let mut s = State::offline();
    s.play_station(&station());
    for id in ["a", "b", "c"] {
        heard(&mut s, id);
    }
    assert_eq!(s.ctl_jump(1), Ok(()), "entry 1 is the older of the two played songs");
    assert_eq!(s.playback.song.as_ref().map(|x| x.id.as_str()), Some("a"));
    assert_eq!(played_ids(&s), ["c", "b"]);
    heard(&mut s, "a"); // MusicKit says the same
    assert_eq!(played_ids(&s), ["c", "b"]);
    // The one-off ends: the station starts again, with its history.
    s.starting = None;
    s.playback.duration = 0.0;
    assert!(s.apply_msg(Msg::Bridge(Event::PlayState(PlayState::Ended))));
    let run = s.queue.station.as_ref().unwrap();
    assert!(!run.one_off);
    assert_eq!(s.playback.state, PlayState::Loading);
    heard(&mut s, "d");
    assert_eq!(played_ids(&s), ["a", "c", "b"]);
}

#[test]
fn repeat_all_does_not_loop_a_replayed_station_song() {
    let mut s = State::offline();
    s.set_repeat(Repeat::All);
    s.play_station(&station());
    for id in ["a", "b"] {
        heard(&mut s, id);
    }
    assert_eq!(s.ctl_jump(0), Ok(()));
    assert!(s.queue.next_lap_songs().is_empty());
    assert!(s.queue.peek_next(Repeat::All).is_none());
}

// ------------------------------------------------------------------------ uzu lyrics

mod uzu_lyrics {
    use super::*;
    use crate::lyrics::{Line, Lyrics, Word};
    use uzumaki_ctl as ctl;

    fn playing(s: &mut State, id: &str) -> Song {
        let song = Song { catalog_id: Some("123".into()), ..song(id) };
        s.play_list(&[song.clone()], 0, "Mix");
        song
    }

    fn sung() -> Lyrics {
        let words = vec![Word { start: 1.0, end: 1.5, text: "la ".into() }, Word { start: 1.5, end: 2.0, text: "di".into() }];
        Lyrics {
            synced: true,
            lines: vec![
                Line { start: 1.0, end: 2.0, text: "la di".into(), second_voice: false, words },
                Line { start: 3.0, end: 4.0, text: "da".into(), second_voice: true, words: vec![] },
            ],
        }
    }

    fn lyric_requests(s: &State) -> usize {
        sent_matching(s, "lyrics")
    }

    /// The reply comes from what the app already holds: no request goes to the bridge, however
    /// often `uzu` asks.
    #[test]
    fn the_reply_comes_from_memory_without_a_bridge_request() {
        let mut s = State::offline();
        s.session.authorized = true;
        let song = playing(&mut s, "i.1");
        assert_eq!(lyric_requests(&s), 1, "the song change asks once (want_lyrics)");
        s.apply_msg(Msg::Lyrics { id: "123".into(), result: Ok(Some(sung())) });
        for _ in 0..3 {
            let ctl::Reply::Lyrics { song_id, status, lines } = s.ctl_lyrics() else { panic!("not lyrics") };
            assert_eq!((song_id.as_str(), status), (song.id.as_str(), ctl::LyricsStatus::Synced));
            assert_eq!(lines.len(), 2);
            assert_eq!(lines[0].words.len(), 2);
            assert_eq!((lines[0].words[0].text.as_str(), lines[0].words[0].start), ("la ", 1.0));
            assert!(lines[1].second_voice && lines[1].words.is_empty());
        }
        assert_eq!(lyric_requests(&s), 1, "answering never asks Apple");
    }

    #[test]
    fn plain_loading_and_none() {
        let mut s = State::offline();
        assert_eq!(s.ctl_lyrics(), ctl::Reply::Lyrics { song_id: String::new(), status: ctl::LyricsStatus::None, lines: vec![] }, "nothing plays");
        s.session.authorized = true;
        let song = playing(&mut s, "i.1");
        let status = |s: &mut State| match s.ctl_lyrics() {
            ctl::Reply::Lyrics { status, lines, .. } => (status, lines.len()),
            _ => panic!("not lyrics"),
        };
        assert_eq!(status(&mut s), (ctl::LyricsStatus::Loading, 0));
        let plain = Lyrics { synced: false, lines: vec![Line { text: "one".into(), ..Default::default() }, Line { text: "two".into(), ..Default::default() }] };
        s.apply_msg(Msg::Lyrics { id: "123".into(), result: Ok(Some(plain)) });
        assert_eq!(status(&mut s), (ctl::LyricsStatus::Plain, 2));
        s.apply_msg(Msg::Lyrics { id: "123".into(), result: Ok(None) });
        assert_eq!(status(&mut s), (ctl::LyricsStatus::None, 0));
        let _ = song;
    }

    /// Missing from memory (cleared): asked once, then `loading` until it lands.
    #[test]
    fn missing_lyrics_are_asked_for_once() {
        let mut s = State::offline();
        s.session.authorized = true;
        playing(&mut s, "i.1");
        s.lyrics.clear();
        let before = lyric_requests(&s);
        for _ in 0..3 {
            assert!(matches!(s.ctl_lyrics(), ctl::Reply::Lyrics { status: ctl::LyricsStatus::Loading, .. }));
        }
        assert_eq!(lyric_requests(&s), before + 1);
    }

    #[test]
    fn a_local_song_reads_its_lrc_file() {
        let dir = std::env::temp_dir().join(format!("uzumaki-lrc-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.lrc"), "[00:01.00]hello\n[00:03.50]world\n").unwrap();
        let mut s = State::offline();
        s.session.authorized = true;
        let with = Song { id: format!("file://{}", dir.join("a.flac").display()), ..local("a") };
        s.play_list(&[with], 0, "Mix");
        let ctl::Reply::Lyrics { status, lines, .. } = s.ctl_lyrics() else { panic!("not lyrics") };
        assert_eq!((status, lines.len(), lines[1].start), (ctl::LyricsStatus::Synced, 2, 3.5));
        assert_eq!(lyric_requests(&s), 0, "never Apple's");
        let without = Song { id: format!("file://{}", dir.join("b.flac").display()), ..local("b") };
        s.play_list(&[without], 0, "Mix");
        assert!(matches!(s.ctl_lyrics(), ctl::Reply::Lyrics { status: ctl::LyricsStatus::None, .. }));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn signed_out_and_local_songs_have_none_and_ask_nothing() {
        let mut s = State::offline();
        playing(&mut s, "i.1");
        s.lyrics.clear();
        assert!(matches!(s.ctl_lyrics(), ctl::Reply::Lyrics { status: ctl::LyricsStatus::None, .. }));
        let before = lyric_requests(&s);
        s.session.authorized = true;
        s.play_list(&[local("x")], 0, "Mix");
        assert!(matches!(s.ctl_lyrics(), ctl::Reply::Lyrics { status: ctl::LyricsStatus::None, .. }));
        assert_eq!(lyric_requests(&s), before);
    }
}

#[test]
fn history_requires_confirmation_survives_contexts_and_keeps_play_next_on_replay() {
    let mut s = State::offline();
    let a = library_song("i.a", "101");
    let b = library_song("i.b", "102");
    let c = library_song("i.c", "103");
    s.play_list(&[a.clone(), b.clone()], 0, "album");
    assert!(s.history.tracks.is_empty());
    s.apply_msg(Msg::Bridge(Event::NowPlaying(Some(a.clone()))));
    s.apply_msg(Msg::Bridge(Event::NowPlaying(Some(a.clone()))));
    assert!(s.history.tracks.is_empty());
    s.apply_msg(Msg::Bridge(Event::NowPlaying(Some(b.clone()))));
    assert_eq!(s.ctl_queue().history.len(), 1);
    s.play_next(&[c.clone()]);
    s.play_history(0);
    assert_eq!(s.queue.pos, 0);
    assert!(s.ctl_queue().history.is_empty(), "optimistic replay isn't shown in both History and current");
    assert_eq!(s.history.tracks.len(), 1, "confirmation history isn't destroyed by optimistic replay");
    assert_eq!(s.queue.user.front(), Some(&c));
    s.apply_msg(Msg::Bridge(Event::NowPlaying(Some(a.clone()))));
    s.play_list(&[c.clone()], 0, "another source");
    s.apply_msg(Msg::Bridge(Event::NowPlaying(Some(c.clone()))));
    assert_eq!(s.ctl_queue().history.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), ["i.b", "i.a"]);
    s.play_history(0);
    assert_eq!(s.playback.song.as_ref().unwrap().id, b.id);
    assert_eq!(s.queue.source, "History");
}

#[test]
fn history_is_chronological_bounded_and_excludes_current_aliases() {
    let mut s = State::offline();
    for i in 0..60 {
        s.history.heard(&library_song(&format!("i.{i}"), &format!("{i}")));
    }
    assert_eq!(s.history.tracks.len(), 50);
    assert_eq!(s.history.tracks.first().unwrap().1.id, "i.9");
    assert_eq!(s.history.tracks.last().unwrap().1.id, "i.58");
    let alias = library_song("other-id", "59");
    s.history.heard(&alias);
    assert_eq!(s.history.tracks.len(), 50);
    s.history.heard(&library_song("i.9", "9"));
    assert!(!s.history.tracks.iter().any(|(_, x)| x.catalog_id.as_deref() == Some("9")));
}

#[test]
fn general_history_from_another_source_replays_once_and_resumes_station() {
    let mut s = State::offline();
    heard(&mut s, "album-song");
    s.play_station(&station());
    heard(&mut s, "radio-song");
    assert_eq!(s.ctl_queue().history[0].id, "album-song");
    s.add_to_queue(&[song("queued")]);
    s.play_history(0);
    assert_eq!(s.playback.song.as_ref().unwrap().id, "album-song");
    assert!(s.queue.station.as_ref().unwrap().one_off);
    assert_eq!(s.queue.user.front().unwrap().id, "queued");
    heard(&mut s, "album-song");
    assert!(!s.ctl_queue().history.iter().any(|s| s.id == "album-song"));
    // Clear the hand-queued song to exercise the same station-resume end path.
    s.queue.user.clear();
    s.starting = None;
    s.playback.duration = 0.0;
    s.apply_msg(Msg::Bridge(Event::PlayState(PlayState::Ended)));
    assert!(!s.queue.station.as_ref().unwrap().one_off);
}

// ------------------------------------------------------------------ queued songs on a station

fn now(s: &State) -> Option<&str> {
    s.playback.song.as_ref().map(|x| x.id.as_str())
}

fn time(s: &mut State, current: f64) -> bool {
    s.apply_msg(Msg::Bridge(Event::Time { current, duration: 200.0 }))
}

/// A station playing song "a", started and confirmed by MusicKit.
fn station_playing() -> State {
    let mut s = State::offline();
    s.play_station(&station());
    heard(&mut s, "a");
    s.apply_msg(Msg::Bridge(Event::PlayState(PlayState::Playing)));
    s
}

fn ended(s: &mut State) -> bool {
    s.starting = None;
    s.playback.duration = 0.0;
    s.apply_msg(Msg::Bridge(Event::PlayState(PlayState::Ended)))
}

#[test]
fn queued_songs_play_when_the_station_song_ends_then_the_station_resumes() {
    let mut s = station_playing();
    s.add_to_queue(&[song("u1"), song("u2")]);
    time(&mut s, 100.0);
    assert_eq!(now(&s), Some("a"), "mid-song nothing changes");
    time(&mut s, 197.0);
    assert_eq!(now(&s), Some("a"), "3 s before the end the station song still plays");
    assert!(time(&mut s, 198.9));
    assert_eq!(now(&s), Some("u1"), "the queued song takes over before the station picks");
    assert!(s.queue.station.as_ref().unwrap().one_off);
    assert_eq!(s.queue.user.len(), 1);
    // Fires once: more time reports near the end change nothing.
    s.apply_msg(Msg::Bridge(Event::PlayState(PlayState::Playing)));
    time(&mut s, 198.0);
    assert_eq!(now(&s), Some("u1"));
    heard(&mut s, "u1");
    heard(&mut s, "u2"); // MusicKit moves on to the second queued song by itself
    assert!(s.queue.user.is_empty());
    assert_eq!(now(&s), Some("u2"));
    // The last queued song ends: the station picks up again, with its history.
    assert!(ended(&mut s));
    assert!(s.queue.station_live());
    assert_eq!(s.playback.state, PlayState::Loading);
    heard(&mut s, "b");
    assert_eq!(played_ids(&s), ["u2", "u1", "a"]);
}

#[test]
fn next_on_a_station_plays_the_queued_song() {
    let mut s = station_playing();
    s.next(); // nothing queued: MusicKit skips within the station (no bridge here)
    assert_eq!(now(&s), Some("a"));
    assert!(s.queue.station_live());
    s.play_next(&[song("u")]);
    s.next();
    assert_eq!(now(&s), Some("u"));
    assert!(s.queue.station.as_ref().unwrap().one_off && s.queue.user.is_empty());
    heard(&mut s, "u");
    assert!(ended(&mut s));
    assert!(s.queue.station_live(), "then the station resumes");
}

#[test]
fn a_station_song_that_slipped_in_gives_way_to_the_queued_one_and_is_not_history() {
    let mut s = station_playing();
    s.add_to_queue(&[song("u")]);
    s.apply_msg(Msg::Bridge(Event::PlayState(PlayState::Paused)));
    time(&mut s, 199.0); // paused near the end: no hand-over
    assert_eq!(now(&s), Some("a"));
    // The station moved on before the hand-over (e.g. a time report missed the last seconds).
    assert!(s.apply_msg(Msg::Bridge(Event::NowPlaying(Some(song("b"))))));
    assert_eq!(now(&s), Some("u"));
    assert!(s.queue.station.as_ref().unwrap().one_off);
    heard(&mut s, "u");
    assert_eq!(played_ids(&s), ["a"], "the discarded station song never counts as played");
}

#[test]
fn a_seek_into_the_end_waits_for_the_seek_before_handing_over() {
    let mut s = station_playing();
    s.add_to_queue(&[song("u")]);
    s.playback.seek_target = Some((199.0, Instant::now()));
    time(&mut s, 50.0); // a report from before the seek
    assert_eq!(now(&s), Some("a"));
    time(&mut s, 199.0); // the seek took: now it is the end of the song
    assert_eq!(now(&s), Some("u"));
}

#[test]
fn play_next_during_a_station_one_off_comes_after_it_then_the_station_resumes() {
    let mut s = station_playing();
    s.add_to_queue(&[song("u1")]);
    s.next();
    heard(&mut s, "u1");
    s.play_next(&[song("p")]);
    assert_eq!(s.queue.upcoming_ids(), ["p"], "MusicKit gets it behind the one-off, as in a list");
    assert!(s.queue.bridge_has_next(Repeat::Off));
    heard(&mut s, "p");
    assert!(s.queue.user.is_empty());
    assert!(ended(&mut s));
    assert!(s.queue.station_live());
}

#[test]
fn a_normal_list_with_queued_songs_never_hands_over_on_time() {
    let mut s = State::offline();
    s.play_list(&[song("x"), song("y")], 0, "Mix");
    s.apply_msg(Msg::Bridge(Event::PlayState(PlayState::Playing)));
    s.add_to_queue(&[song("u")]);
    assert_eq!(s.queue.upcoming_ids(), ["u", "y"]);
    assert!(s.queue.bridge_has_next(Repeat::Off));
    time(&mut s, 199.0);
    assert_eq!(now(&s), Some("x"));
    assert_eq!(s.queue.user.len(), 1);
    heard(&mut s, "u");
    assert!(s.queue.user.is_empty() && s.queue.station.is_none());
}

#[test]
fn the_station_hand_over_window_is_one_report_wide_at_either_time_rate() {
    // 1 report/s: the window is a bit over a second, so one report always falls in it.
    let mut s = station_playing();
    s.add_to_queue(&[song("u")]);
    // (Reports more than a second apart, so the clock jumps to each instead of slewing.)
    time(&mut s, 197.5);
    assert_eq!(now(&s), Some("a"), "2.5 s left: not yet");
    time(&mut s, 198.8);
    assert_eq!(now(&s), Some("u"), "1.2 s left at 1 report/s: the next report could come too late");
    // 4 reports/s (synced lyrics on screen): the station song plays until 1 s before its end.
    let mut s = station_playing();
    s.set_fine_time(true);
    s.add_to_queue(&[song("u")]);
    time(&mut s, 197.5);
    time(&mut s, 198.8);
    assert_eq!(now(&s), Some("a"), "1.2 s left at 4 reports/s: more reports will come");
    time(&mut s, 199.9);
    assert_eq!(now(&s), Some("u"));
}

// ------------------------------------------------------- the end of the last queued song on a station

/// A station plays "a"; three songs are queued and the first takes over at the end of "a".
/// MusicKit then moves on to u2 and u3 by itself; u3 (the last) plays, `duration` long.
fn station_on_its_last_queued_song(duration: f64) -> State {
    let mut s = station_playing();
    s.add_to_queue(&[song("u1"), song("u2")]);
    s.play_next(&[song("p")]);
    assert!(time(&mut s, 199.2), "hand-over to the first queued song");
    assert_eq!(now(&s), Some("p"));
    heard(&mut s, "p");
    s.apply_msg(Msg::Bridge(Event::PlayState(PlayState::Playing)));
    heard(&mut s, "u1");
    let last = Song { duration, ..song("u2") };
    s.apply_msg(Msg::Bridge(Event::NowPlaying(Some(last))));
    s.apply_msg(Msg::Bridge(Event::PlayState(PlayState::Playing)));
    assert!(s.queue.user.is_empty() && s.queue.station.as_ref().unwrap().one_off);
    assert!(s.queue.upcoming_ids().is_empty() && s.queue.cycle_ids().is_none(), "the bridge has nothing to top up");
    s
}

fn resumed(s: &State) -> bool {
    s.queue.station_live() && s.playback.state == PlayState::Loading
}

#[test]
fn the_last_queued_song_ending_at_its_end_resumes_the_station_for_every_end_state() {
    for end in [PlayState::Ended, PlayState::Completed, PlayState::Stopped] {
        let mut s = station_on_its_last_queued_song(200.0);
        time(&mut s, 199.0);
        assert!(s.apply_msg(Msg::Bridge(Event::PlayState(end))), "{end:?}");
        assert!(resumed(&s), "{end:?}");
        assert_eq!(played_ids(&s), ["u1", "p", "a"]);
    }
}

#[test]
fn the_last_queued_song_ending_well_before_its_listed_duration_resumes_the_station() {
    // Live 2026-11-10: a 973 s library mix stopped at 943 s; the app stayed on "stopped".
    for end in [PlayState::Ended, PlayState::Completed, PlayState::Stopped] {
        let mut s = station_on_its_last_queued_song(973.0);
        s.apply_msg(Msg::Bridge(Event::Time { current: 943.3, duration: 973.0 }));
        assert!(s.apply_msg(Msg::Bridge(Event::PlayState(end))), "{end:?}");
        assert!(resumed(&s), "{end:?}");
    }
}

#[test]
fn musickits_queue_running_out_on_the_last_queued_song_resumes_the_station() {
    let mut s = station_on_its_last_queued_song(973.0);
    s.apply_msg(Msg::Bridge(Event::Time { current: 943.3, duration: 973.0 }));
    assert!(s.apply_msg(Msg::Bridge(Event::NowPlaying(None))));
    assert!(resumed(&s));
    // The station's own start (its setQueue clears MusicKit's song) changes nothing.
    assert!(!s.apply_msg(Msg::Bridge(Event::NowPlaying(None))));
    assert!(s.queue.station_live());
}

#[test]
fn the_song_befores_late_end_state_does_not_resume_the_station() {
    let mut s = station_on_its_last_queued_song(200.0);
    // u2 just started (its nowPlaying set the position to 0); u1's "stopped" comes late.
    s.apply_msg(Msg::Bridge(Event::PlayState(PlayState::Stopped)));
    assert!(!s.queue.station_live() && now(&s) == Some("u2"));
    let mut s = station_on_its_last_queued_song(200.0);
    s.apply_msg(Msg::Bridge(Event::NowPlaying(None)));
    assert!(!s.queue.station_live());
}

#[test]
fn a_paused_last_queued_song_that_musickit_stops_does_not_start_the_station() {
    let mut s = station_on_its_last_queued_song(973.0);
    s.apply_msg(Msg::Bridge(Event::Time { current: 500.0, duration: 973.0 }));
    s.apply_msg(Msg::Bridge(Event::PlayState(PlayState::Paused)));
    s.apply_msg(Msg::Bridge(Event::PlayState(PlayState::Stopped)));
    assert!(!s.queue.station_live(), "the user paused: no music starts on its own");
}

#[test]
fn next_on_the_last_queued_song_resumes_the_station() {
    let mut s = station_on_its_last_queued_song(200.0);
    time(&mut s, 20.0);
    s.next();
    assert!(resumed(&s));
}

#[test]
fn picking_an_app_icon_changes_the_setting_and_touches_no_files_offline() {
    use crate::settings::AppIcon;
    // No test ever starts a persisting state, so nothing may ask for the icon files (Linux) or
    // the .app bundle's icon (macOS) to change.
    #[cfg(target_os = "macos")]
    let requests = crate::bundle_icon::requests;
    #[cfg(not(target_os = "macos"))]
    let requests = crate::app_icon::requests;
    let before = requests();
    let mut s = State::offline();
    assert_eq!(s.settings.app_icon, AppIcon::Dark);
    s.set_app_icon(AppIcon::Coral); // offline state never writes settings or icon files
    assert_eq!(s.settings.app_icon, AppIcon::Coral);
    s.set_app_icon(AppIcon::Dark);
    assert_eq!(s.settings.app_icon, AppIcon::Dark);
    assert_eq!(requests(), before, "no icon was applied");
}

#[test]
fn follow_theme_applies_the_icon_only_when_it_changes_and_touches_no_files_offline() {
    use crate::settings::{AppIcon, IconVariant, Theme};
    #[cfg(target_os = "macos")]
    let requests = crate::bundle_icon::requests;
    #[cfg(not(target_os = "macos"))]
    let requests = crate::app_icon::requests;
    let before = requests();
    let mut s = State::offline();
    assert_eq!(s.settings.theme, Theme::System);
    assert_eq!(s.applied_icon, Some(IconVariant::Dark), "the start applies the default icon");
    assert_eq!(s.icon_applies, 1);

    // Follow theme before the first frame: the appearance is unknown, nothing is applied.
    s.set_app_icon(AppIcon::Auto);
    assert_eq!(s.settings.app_icon, AppIcon::Auto, "the stored choice stays Auto");
    assert_eq!((s.applied_icon, s.icon_applies), (Some(IconVariant::Dark), 1));
    // The view reports a dark system: still the Dark icon, no work.
    s.set_system_dark(true);
    assert_eq!(s.icon_applies, 1);
    // The system turns light: the Light icon, once; repeated reports do nothing.
    s.set_system_dark(false);
    s.set_system_dark(false);
    assert_eq!((s.applied_icon, s.icon_applies), (Some(IconVariant::Light), 2));
    assert_eq!(s.shown_app_icon(), IconVariant::Light);
    // A forced theme wins over the system; switching to the theme it already shows does nothing.
    s.set_theme(Theme::Light);
    assert_eq!(s.icon_applies, 2);
    s.set_theme(Theme::Dark);
    assert_eq!((s.applied_icon, s.icon_applies), (Some(IconVariant::Dark), 3));
    s.set_system_dark(false); // ignored while the theme is forced
    assert_eq!(s.icon_applies, 3);
    // A fixed choice ignores the theme.
    s.set_app_icon(AppIcon::Coral);
    s.set_theme(Theme::Light);
    assert_eq!((s.applied_icon, s.icon_applies), (Some(IconVariant::Coral), 4));
    assert_eq!(s.settings.app_icon, AppIcon::Coral);
    // Offline: no file or bundle work was ever scheduled, and the view posts nothing (no handle).
    s.report_system_dark(true);
    assert_eq!(requests(), before, "no icon was applied");
}

// ---------------------------------------------------------------- local only mode (onboarding stage 2)

use crate::settings::MusicSource;

/// A state whose engine starts stand-in bridges; the counter counts starts.
fn with_engine() -> (State, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
    let mut s = State::offline();
    let (engine, starts) = crate::bridge::engine_tests::fake::<Msg>(|_| {});
    s.engine = Some(engine);
    (s, starts)
}

fn started(n: &std::sync::atomic::AtomicUsize) -> usize {
    n.load(std::sync::atomic::Ordering::SeqCst)
}

fn running(s: &State) -> bool {
    s.engine.as_ref().is_some_and(|e| e.running())
}

#[test]
fn local_only_never_starts_the_bridge() {
    let (mut s, starts) = with_engine();
    s.settings.music_source = MusicSource::LocalOnly;
    s.start_bridge();
    assert!(!s.start_bridge_with(true));
    assert_eq!(started(&starts), 0);
    assert!(s.bridge.is_none() && !running(&s));
    // A launch in local only mode: no bridge, no sync screen even with nothing on disk.
    crate::ui::onboarding::launch(&mut s, false, false, true, false);
    assert_eq!(started(&starts), 0);
    assert!(!crate::ui::intro::syncing(&s));
}

#[test]
fn a_launch_starts_the_bridge_for_apple_but_not_under_the_onboarding() {
    let (mut s, starts) = with_engine();
    crate::ui::onboarding::launch(&mut s, true, false, true, false);
    assert!(s.ui.onboarding.is_some());
    assert_eq!(started(&starts), 0, "a new install waits for the choice");
    let (mut s, starts) = with_engine();
    crate::ui::onboarding::launch(&mut s, false, false, true, true);
    assert_eq!(started(&starts), 1);
    assert!(s.bridge.is_some() && s.session.login_window, "--login shows the sign-in window");
    s.start_bridge();
    assert_eq!(started(&starts), 1, "one bridge at a time");
    s.engine.as_ref().unwrap().shutdown();
}

#[test]
fn switching_sources_stops_and_starts_the_bridge_and_keeps_the_sign_in() {
    let (mut s, starts) = with_engine();
    s.start_bridge();
    s.session.bridge_up = true;
    s.session.authorized = true;
    s.session.storefront = Some("it".into());
    s.navigate(Route::Library(LibraryTab::Albums));
    s.navigate(Route::Search);
    s.set_music_source(MusicSource::LocalOnly);
    assert_eq!(s.settings.music_source, MusicSource::LocalOnly);
    assert!(s.bridge.is_none() && !running(&s), "stopped");
    assert!(!s.session.bridge_up && s.session.error.is_none());
    assert!(s.session.authorized && s.session.storefront.is_some(), "the sign-in is kept (decision 4)");
    assert_eq!(s.route, Route::Search, "a page local only has stays");
    assert!(s.back_stack.iter().all(|r| !r.needs_apple()), "no way back to an Apple page");
    assert_eq!(s.ui.search_tab, crate::ui::SearchTab::Local);
    // Apple pages can't be opened, and Back skips them.
    s.navigate(Route::Home);
    assert_eq!(s.route, Route::Search);
    s.navigate(Route::LocalFiles);
    assert_eq!(s.route, Route::LocalFiles);
    // The old bridge's last word changes nothing.
    assert!(!s.apply_msg(Msg::Bridge(Event::Stopped)));
    assert!(s.session.error.is_none());

    // Back to Apple: a new bridge, and (not signed in) the guided sign-in step on its own; Apple's
    // window waits for its button (stage 3).
    s.session.authorized = false;
    s.switch_to_apple();
    assert_eq!(started(&starts), 2);
    assert!(s.bridge.is_some() && !s.session.login_after_ready);
    assert!(crate::ui::onboarding::active(&s));
    s.apply_msg(Msg::Bridge(Event::Ready { authorized: false, storefront: Some("it".into()), volume: None }));
    assert!(!s.session.login_window);
    s.ui.onboarding = None;
    s.navigate(Route::Home);
    assert_eq!(s.route, Route::Home);
    s.engine.as_ref().unwrap().shutdown();
}

#[test]
fn switching_to_local_only_stops_an_apple_song_and_keeps_a_playing_file() {
    // An Apple song plays: it stops, and the queue empties.
    let mut s = State::offline();
    s.play_list(&[song("a"), local("x"), song("b")], 0, "Mix");
    assert!(!s.playback.local);
    s.set_music_source(MusicSource::LocalOnly);
    assert!(s.playback.song.is_none());
    assert_eq!(s.queue.up_next_len(), 0);

    // A file plays: it goes on; the Apple songs leave the queue, the files stay in order.
    let mut s = State::offline();
    s.play_list(&[song("a"), local("x"), song("b"), local("y")], 1, "Mix");
    s.queue.add_to_queue(&[song("c"), local("z")]);
    assert!(s.playback.local);
    s.set_music_source(MusicSource::LocalOnly);
    assert_eq!(s.playback.song.as_ref().map(|x| x.title.as_str()), Some("x"));
    assert!(s.playback.local);
    let next: Vec<&str> = s.queue.up_next().map(|x| x.title.as_str()).collect();
    assert_eq!(next, ["z", "y"]);
    assert_eq!(s.queue.current().map(|x| x.title.as_str()), Some("x"));
    // An Apple song reached anyway (the history) doesn't start.
    let before = s.playback.song.clone();
    s.begin_current(song("q"));
    assert_eq!(s.playback.song, before);
    assert!(s.toast.is_some());
}

#[test]
fn uzu_commands_that_need_apple_say_local_only_mode() {
    use crate::ctl::needs_apple;
    use uzumaki_ctl::Request;
    assert_eq!(needs_apple(&Request::Search { query: "x".into() }), Some("uzu search"));
    assert_eq!(needs_apple(&Request::Playlists), Some("uzu playlists"));
    assert_eq!(needs_apple(&Request::Lyrics), Some("uzu lyrics"));
    assert_eq!(needs_apple(&Request::Status), None);
    assert_eq!(needs_apple(&Request::Toggle), None);
    assert_eq!(needs_apple(&Request::Queue), None);
    let msg = crate::state::local_only_answer("uzu search");
    assert!(msg.contains("local only mode") && msg.contains("uzu search") && msg.contains("Settings"), "{msg}");

    // The bridge for Apple calls comes from the state, so a late one is seen.
    let (mut s, _starts) = with_engine();
    assert_eq!(s.apple_bridge().err().as_deref(), Some("the playback bridge isn't running"));
    s.start_bridge();
    assert!(s.apple_bridge().is_ok(), "started after launch: seen");
    s.set_music_source(MusicSource::LocalOnly);
    assert!(s.apple_bridge().err().unwrap().contains("local only mode"));
}

#[test]
fn number_keys_skip_the_apple_pages_in_local_only_mode() {
    let mut s = State::offline();
    assert_eq!(crate::ui::slot_target(&s, 1).as_deref(), Some("Home"));
    s.set_music_source(MusicSource::LocalOnly);
    // Ctrl+1 is still the songs (Local Files, the Songs tab now), then the other tabs.
    assert_eq!(crate::ui::slot_target(&s, 1).as_deref(), Some("Songs"));
    assert_eq!(crate::ui::slot_target(&s, 2).as_deref(), Some("Artists"));
    assert_eq!(crate::ui::slot_target(&s, 4).as_deref(), Some("Folders"));
    assert_eq!(crate::ui::slot_target(&s, 5), None);
}

// ---------------------------------------------------------------- local only library tabs (stage 2b)

mod local_tabs {
    use super::*;
    use crate::local::view::{self, tests::track};
    use std::path::PathBuf;

    /// Local only mode over a small music folder `/m`: an album on two discs, a compilation with a
    /// guest, and an untagged folder with a loose file next to it.
    fn local_state() -> State {
        let mut s = State::offline();
        s.settings.music_folders = vec![PathBuf::from("/m")];
        s.set_music_source(MusicSource::LocalOnly);
        let tracks = [
            track("/m/Ann/Debut/2-01.flac", "Encore", "Ann", "Debut", "", Some(2), Some(1)),
            track("/m/Ann/Debut/1-01.flac", "Opening", "Ann", "Debut", "", Some(1), Some(1)),
            track("/m/Ann/Debut/1-02.flac", "Middle", "Ann", "Debut", "", Some(1), Some(2)),
            track("/m/Comp/1.flac", "Guest Spot", "Ann", "Hits", "Various Artists", None, Some(1)),
            track("/m/Comp/2.flac", "Other", "Bo", "Hits", "Various Artists", None, Some(2)),
            track("/m/Rips/tape/side a.mp3", "side a", "Unknown Artist", "tape", "Unknown Artist", None, None),
            track("/m/Rips/loose.wav", "loose", "Unknown Artist", "Rips", "Unknown Artist", None, None),
        ];
        let (songs, v) = view::from_index(&tracks, &s.settings.music_folders);
        s.set_local_index(songs, v);
        s
    }

    fn album_id(s: &State, title: &str) -> String {
        s.local_lib.view.albums.iter().find(|c| c.title == title).unwrap().id.clone()
    }

    fn titles(songs: &[Song]) -> Vec<&str> {
        songs.iter().map(|x| x.title.as_str()).collect()
    }

    #[test]
    fn a_local_album_opens_on_the_album_page_with_its_songs_in_disc_order() {
        let mut s = local_state();
        let id = album_id(&s, "Debut");
        let card = s.local_lib.view.albums.iter().find(|c| c.id == id).unwrap().clone();
        assert!(card.tracks.is_empty() && card.track_count == Some(3), "the grid keeps no song copies");
        s.open_collection(&card);
        assert_eq!(s.route, Route::Collection { kind: CollectionKind::Album, id: id.clone() });
        let c = s.collections[&id].ready().unwrap();
        assert_eq!(titles(&c.tracks), ["Opening", "Middle", "Encore"]);
        assert_eq!(c.artists[0].id, view::artist_id("Ann"));
        assert_eq!(sent_matching(&s, "/v1/"), 0, "nothing asked of Apple");
        // A new scan drops the old copy and fills the open page again.
        let (songs, v) = view::from_index(&[track("/m/Ann/Debut/1-01.flac", "Opening", "Ann", "Debut", "", Some(1), Some(1))], &s.settings.music_folders);
        s.set_local_index(songs, v);
        assert_eq!(titles(&s.collections[&id].ready().unwrap().tracks), ["Opening"]);
    }

    #[test]
    fn a_card_plays_and_shuffles_without_its_songs_loaded() {
        let mut s = local_state();
        let card = s.local_lib.view.albums.iter().find(|c| c.title == "Debut").unwrap().clone();
        s.play_collection(&card, 0);
        assert_eq!(local_sent(&s), ["play Opening"]);
        assert_eq!(s.queue.up_next().map(|x| x.title.as_str()).collect::<Vec<_>>(), ["Middle", "Encore"]);
    }

    #[test]
    fn the_artist_page_lists_their_songs_and_guest_spots() {
        let mut s = local_state();
        s.navigate(Route::Artist { id: view::artist_id("ann"), name: "Ann".into() });
        assert!(!s.route.needs_apple());
        let songs = s.local_lib.page_songs(&s.route).to_vec();
        assert_eq!(titles(&songs), ["Opening", "Middle", "Encore", "Guest Spot"]);
        let v = &s.local_lib.view;
        let a = v.artist(&view::artist_id("Ann")).unwrap();
        assert_eq!(v.artist_albums[a].len(), 1, "the compilation is Various Artists'");
    }

    #[test]
    fn folders_list_subfolders_then_files_and_play_everything_below() {
        let mut s = local_state();
        s.navigate(Route::Folders { path: Some("/m/Rips".into()) });
        assert_eq!(titles(s.local_lib.page_songs(&s.route)), ["loose"], "the page lists its own files");
        let f = s.local_lib.view.folder(std::path::Path::new("/m/Rips")).unwrap();
        assert_eq!(f.dirs, [PathBuf::from("/m/Rips/tape")]);
        assert_eq!(titles(&s.local_songs(&LocalWhat::Folder("/m/Rips".into()))), ["loose", "side a"]);
        let all = s.local_songs(&LocalWhat::Folder("/m".into()));
        assert_eq!(all.len(), 7);
        // Add to Queue with nothing playing starts the first song, the rest queued behind it.
        s.add_to_queue(&s.local_songs(&LocalWhat::Folder("/m/Rips".into())));
        let queued: Vec<&str> = s.playback.song.iter().chain(s.queue.up_next()).map(|x| x.title.as_str()).collect();
        assert_eq!(queued, ["loose", "side a"]);
    }

    #[test]
    fn go_to_album_and_artist_open_the_local_pages_in_local_only_mode() {
        let mut s = local_state();
        let song = s.local_lib.songs.iter().find(|x| x.title == "side a").unwrap().clone();
        s.go_to_album(&song);
        assert_eq!(s.route, Route::Collection { kind: CollectionKind::Album, id: album_id(&s, "tape") });
        s.go_to_local_artist(&song);
        assert_eq!(s.route, Route::Artist { id: view::artist_id("Unknown Artist"), name: "Unknown Artist".into() });
    }

    #[test]
    fn apple_mode_is_unchanged_and_leaves_the_local_tabs() {
        let mut s = local_state();
        s.navigate(Route::LocalAlbums);
        s.set_music_source(MusicSource::Apple);
        assert_eq!(s.route, Route::LocalFiles, "Albums of the music folders are local only mode's");
        assert!(s.back_stack.iter().all(|r| !r.local_library()));
        // Go to Album on a file still opens Local Files there.
        let song = s.local_lib.songs[0].clone();
        s.navigate(Route::Home);
        s.go_to_album(&song);
        assert_eq!(s.route, Route::LocalFiles);
        // Apple pages need Apple; the local ones don't, and Apple ids are still Apple's.
        assert!(Route::Library(LibraryTab::Albums).needs_apple());
        assert!(Route::Collection { kind: CollectionKind::Album, id: "l.1".into() }.needs_apple());
        assert!(Route::Artist { id: "r.1".into(), name: "A".into() }.needs_apple());
        assert!(!Route::LocalAlbums.needs_apple() && !Route::Folders { path: None }.needs_apple());
        // A plain Apple album page fills nothing local.
        s.navigate(Route::Collection { kind: CollectionKind::Album, id: "1440".into() });
        assert!(s.local_lib.page.as_ref().is_none_or(|(r, _)| *r != s.route));
    }

    #[test]
    fn removing_a_music_folder_keeps_the_tabs_in_step() {
        let mut s = local_state();
        s.settings.music_folders.push("/n".into());
        let mut extra = s.local_lib.songs.clone();
        extra.push(Song { id: "file:///n/x.flac".into(), ..local("x") });
        let mut aa: Vec<String> = s.local_lib.songs.iter().enumerate().map(|(i, _)| {
            let v = &s.local_lib.view;
            v.albums[v.song_album[i] as usize].subtitle.clone()
        }).collect();
        aa.push(String::new());
        let v = view::derive(&mut extra, &aa, &s.settings.music_folders);
        s.set_local_index(extra, v);
        assert_eq!(s.local_lib.view.albums.len(), 5);
        s.remove_music_folder(1);
        assert_eq!(s.local_lib.songs.len(), 7);
        assert_eq!(s.local_lib.view.albums.len(), 4);
        assert_eq!(s.local_lib.view.song_album.len(), 7);
        let a = s.local_lib.view.album(&album_id(&s, "Hits")).unwrap();
        assert_eq!(s.local_lib.view.albums[a].subtitle, "Various Artists", "album artists survive the rebuild");
    }

    #[test]
    fn uzu_lists_and_plays_local_albums_in_local_only_mode() {
        let mut s = local_state();
        let albums = s.ctl_albums();
        assert_eq!(albums.iter().map(|a| a.title.as_str()).collect::<Vec<_>>(), ["Debut", "Hits", "Rips", "tape"]);
        assert_eq!(albums[0].count, Some(3));
        let (loaded, _) = s.ctl_lookup(&albums[0].id);
        let c = loaded.unwrap();
        assert_eq!(c.tracks.len(), 3);
        s.ctl_play_collection(&c, Some(1), None).unwrap();
        assert_eq!(local_sent(&s), ["play Middle"]);
    }
}


// ---------------------------------------------------------------- storefront (onboarding stage 3)

fn storefront_asks(s: &State) -> Vec<String> {
    s.sent.borrow().iter().filter(|l| l.contains("storefront")).cloned().collect()
}

/// Signed in, the storefront that came with the sign-in is saved (and its name asked for once);
/// a later `storefront` event replaces it; signing out forgets it.
#[test]
fn the_storefront_is_saved_after_sign_in() {
    let mut s = State::offline();
    s.apply_msg(Msg::Bridge(Event::Ready { authorized: false, storefront: Some("us".into()), volume: None }));
    assert_eq!(s.session.storefront.as_deref(), Some("us"), "the page's, for its own requests");
    assert_eq!(s.settings.storefront, None, "not an account's");
    assert!(storefront_asks(&s).is_empty());

    s.apply_msg(Msg::Bridge(Event::Auth { authorized: true, storefront: Some("it".into()) }));
    assert_eq!(s.settings.storefront, Some(crate::settings::Storefront { id: "it".into(), name: None }));
    assert_eq!(s.session.storefront.as_deref(), Some("it"));
    assert_eq!(storefront_asks(&s), ["/v1/storefronts/it {}"], "its name, once");
    s.apply_msg(Msg::Storefront(Ok(("it".into(), Some("Italy".into())))));
    assert_eq!(s.settings.storefront.as_ref().unwrap().label(), "Italy");
    // The same again (a page reload's `ready`): nothing new asked, the name kept.
    s.apply_msg(Msg::Bridge(Event::Ready { authorized: true, storefront: Some("it".into()), volume: None }));
    assert_eq!(storefront_asks(&s).len(), 1);
    assert_eq!(s.settings.storefront.as_ref().unwrap().label(), "Italy");

    // MusicKit switches storefront afterwards.
    s.apply_msg(Msg::Bridge(Event::Storefront("jp".into())));
    assert_eq!(s.settings.storefront, Some(crate::settings::Storefront { id: "jp".into(), name: None }));
    assert_eq!(s.session.storefront.as_deref(), Some("jp"));

    // Signed out: forgotten.
    s.apply_msg(Msg::Bridge(Event::Auth { authorized: false, storefront: None }));
    assert_eq!(s.settings.storefront, None);
    // A reply that lands after signing out is dropped.
    s.apply_msg(Msg::Storefront(Ok(("jp".into(), Some("Japan".into())))));
    assert_eq!(s.settings.storefront, None);
}

/// No storefront came with the sign-in (an older bridge, or MusicKit didn't know it yet): Apple is
/// asked once (`/v1/me/storefront`), and its answer saved with the name.
#[test]
fn the_storefront_falls_back_to_asking_apple_once() {
    let mut s = State::offline();
    s.apply_msg(Msg::Bridge(Event::Auth { authorized: true, storefront: None }));
    assert_eq!(storefront_asks(&s), ["/v1/me/storefront {}"]);
    s.apply_msg(Msg::Bridge(Event::Ready { authorized: true, storefront: None, volume: None }));
    assert_eq!(storefront_asks(&s).len(), 1, "once");
    s.apply_msg(Msg::Storefront(Err("offline".into())));
    assert_eq!(s.settings.storefront, None);
    s.apply_msg(Msg::Storefront(Ok(("it".into(), Some("Italy".into())))));
    assert_eq!(s.settings.storefront.as_ref().map(|f| f.label()), Some("Italy".into()));
    assert_eq!(storefront_asks(&s).len(), 1, "the answer had the name");
    // A saved storefront needs no fallback.
    let mut s = State::offline();
    s.settings.storefront = Some(crate::settings::Storefront { id: "it".into(), name: Some("Italy".into()) });
    s.apply_msg(Msg::Bridge(Event::Auth { authorized: true, storefront: None }));
    assert!(storefront_asks(&s).is_empty());
}

#[test]
fn the_storefront_reply_reads_apple_s_list() {
    let v = serde_json::json!({"data": [{"id": "it", "type": "storefronts", "attributes": {"name": "Italy", "defaultLanguageTag": "it-IT"}}]});
    assert_eq!(super::signin::parse_storefront_for_tests(&v), Some(("it".into(), Some("Italy".into()))));
    assert_eq!(super::signin::parse_storefront_for_tests(&v["data"]), Some(("it".into(), Some("Italy".into()))));
    assert_eq!(super::signin::parse_storefront_for_tests(&serde_json::json!({"data": []})), None);
}
