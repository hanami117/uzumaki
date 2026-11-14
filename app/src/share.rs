//! Share → Copy Link: the music.apple.com page of a song, album, playlist, artist or station.
//!
//! Apple's own menus (Music on the Mac, the web player) have Share ▸ Copy Link on all of them.
//! The link is the item's own `attributes.url` when Apple sent one (remembered while the
//! response is parsed, so the models stay as they are); otherwise it is built with the user's
//! storefront, in Apple's shapes:
//!
//! - album `/album/<slug>/<id>`, song = its album's page + `?i=<songId>`
//! - playlist `/playlist/<slug>/pl.<id>`, artist `/artist/<slug>/<id>`, station `/station/<slug>/ra.<id>`
//!
//! The slug is cosmetic: music.apple.com redirects any slug (even `_`) to the canonical one.
//! Library items need their catalog twin: a library playlist carries it (`playParams.globalId`),
//! library albums and artists are looked up (`/v1/me/library/<kind>/<id>/catalog`) when the item
//! is clicked. Local files, personal stations and the user's private playlists have no page.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use serde_json::Value;

use crate::models::{Artist, Collection, CollectionKind, Song, Station};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Song,
    Album,
    Playlist,
    Artist,
    Station,
}

impl Kind {
    fn path(self) -> &'static str {
        match self {
            Kind::Song => "song",
            Kind::Album => "album",
            Kind::Playlist => "playlist",
            Kind::Artist => "artist",
            Kind::Station => "station",
        }
    }
}

/// What the menu can offer for an item.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Link {
    /// Ready to copy.
    Url(String),
    /// A library item whose catalog page must be asked for first.
    Lookup { kind: Kind, id: String, title: String },
    /// No page exists (or none is known): the menu has no Share.
    Hidden,
}

#[derive(Clone, Debug)]
enum Known {
    /// Apple's own `attributes.url`.
    Url(String),
    /// The catalog id of a library item.
    Catalog(String),
}

/// A bound on what is remembered: a long session browsing the catalog never grows it past this.
const CAP: usize = 50_000;

fn known() -> &'static Mutex<HashMap<(Kind, String), Known>> {
    static KNOWN: OnceLock<Mutex<HashMap<(Kind, String), Known>>> = OnceLock::new();
    KNOWN.get_or_init(Default::default)
}

fn put(kind: Kind, id: &str, k: Known) {
    let mut m = known().lock().unwrap_or_else(|e| e.into_inner());
    if m.len() >= CAP {
        m.clear();
    }
    m.insert((kind, id.to_string()), k);
}

fn get(kind: Kind, id: &str) -> Option<Known> {
    known().lock().unwrap_or_else(|e| e.into_inner()).get(&(kind, id.to_string())).cloned()
}

/// Remembers what a parsed API resource says about its page: its `attributes.url` (https
/// music.apple.com only), or the catalog id behind a library playlist (`playParams.globalId`).
pub fn note(kind: Kind, v: &Value) {
    let Some(id) = v["id"].as_str() else { return };
    let a = &v["attributes"];
    if let Some(url) = a["url"].as_str().filter(|u| is_apple_url(u)) {
        put(kind, id, Known::Url(url.to_string()));
    } else if let Some(g) = a["playParams"]["globalId"].as_str().filter(|g| !g.is_empty()) {
        put(kind, id, Known::Catalog(g.to_string()));
    }
}

fn is_apple_url(u: &str) -> bool {
    u.starts_with("https://music.apple.com/")
}

/// The slug music.apple.com puts in a page's path: lower-case, runs of anything but letters and
/// digits become one hyphen. `_` when nothing is left (Apple redirects it to the real slug).
pub fn slug(title: &str) -> String {
    let mut out = String::new();
    for c in title.chars().flat_map(char::to_lowercase) {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    let out = out.trim_end_matches('-');
    if out.is_empty() { "_".into() } else { out.into() }
}

fn page(storefront: Option<&str>, kind: Kind, title: &str, id: &str) -> String {
    format!("https://music.apple.com/{}/{}/{}/{id}", storefront.unwrap_or("us"), kind.path(), slug(title))
}

fn is_catalog_number(id: &str) -> bool {
    !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit())
}

fn from_known(kind: Kind, id: &str, title: &str, storefront: Option<&str>) -> Option<String> {
    match get(kind, id)? {
        Known::Url(u) => Some(u),
        Known::Catalog(c) => Some(page(storefront, kind, title, &c)),
    }
}

pub fn song_link(song: &Song, storefront: Option<&str>) -> Link {
    let Some(id) = song.catalog_ref() else { return Link::Hidden };
    if let Some(u) = from_known(Kind::Song, id, &song.title, storefront).or_else(|| from_known(Kind::Song, &song.id, &song.title, storefront)) {
        return Link::Url(u);
    }
    // A song is its album's page with `?i=`; without the album, Apple's /song/ path redirects there.
    Link::Url(match song.album_id.as_deref().filter(|a| is_catalog_number(a)) {
        Some(album) => format!("{}?i={id}", page(storefront, Kind::Album, &song.album, album)),
        None => page(storefront, Kind::Song, &song.title, id),
    })
}

pub fn collection_link(c: &Collection, storefront: Option<&str>) -> Link {
    if c.favorites || crate::local::view::is_album_id(&c.id) {
        return Link::Hidden;
    }
    let kind = match c.kind {
        CollectionKind::Album => Kind::Album,
        CollectionKind::Playlist => Kind::Playlist,
    };
    if let Some(u) = from_known(kind, &c.id, &c.title, storefront) {
        // A private playlist of the user's own isn't there for others, whatever its global id.
        return if c.kind == CollectionKind::Playlist && c.editable && c.public == Some(false) { Link::Hidden } else { Link::Url(u) };
    }
    match (kind, c.id.as_str()) {
        (Kind::Album, id) if is_catalog_number(id) => Link::Url(page(storefront, kind, &c.title, id)),
        (Kind::Playlist, id) if id.starts_with("pl.") => Link::Url(page(storefront, kind, &c.title, id)),
        (Kind::Album, id) if id.starts_with("l.") => Link::Lookup { kind, id: id.into(), title: c.title.clone() },
        _ => Link::Hidden,
    }
}

pub fn artist_link(a: &Artist, storefront: Option<&str>) -> Link {
    if crate::local::view::is_artist_id(&a.id) {
        return Link::Hidden;
    }
    if let Some(u) = from_known(Kind::Artist, &a.id, &a.name, storefront) {
        return Link::Url(u);
    }
    if is_catalog_number(&a.id) {
        Link::Url(page(storefront, Kind::Artist, &a.name, &a.id))
    } else if Artist::is_library_id(&a.id) {
        Link::Lookup { kind: Kind::Artist, id: a.id.clone(), title: a.name.clone() }
    } else {
        Link::Hidden
    }
}

pub fn station_link(st: &Station, storefront: Option<&str>) -> Link {
    link_of_station(&st.id, &st.name, storefront)
}

/// `ra.u-…` is the user's own station (no public page); `ra.<number>` are Apple's.
pub fn link_of_station(id: &str, name: &str, storefront: Option<&str>) -> Link {
    if !id.starts_with("ra.") || id.starts_with("ra.u-") {
        return Link::Hidden;
    }
    Link::Url(from_known(Kind::Station, id, name, storefront).unwrap_or_else(|| page(storefront, Kind::Station, name, id)))
}

/// The library request that answers a `Link::Lookup`: the catalog resource of a library item.
pub fn lookup_path(kind: Kind, id: &str) -> Option<String> {
    let what = match kind {
        Kind::Album => "albums",
        Kind::Artist => "artists",
        Kind::Song => "songs",
        _ => return None,
    };
    Some(format!("/v1/me/library/{what}/{id}/catalog"))
}

/// Apple's answer to a lookup: the catalog resource's id and url, when it has one.
pub fn parse_lookup(v: &Value) -> Option<(String, Option<String>)> {
    let d = v["data"].get(0)?;
    let url = d["attributes"]["url"].as_str().filter(|u| is_apple_url(u)).map(String::from);
    Some((d["id"].as_str()?.to_string(), url))
}

/// Keeps a lookup's answer for the next click and returns the link it makes.
pub fn resolved(kind: Kind, library_id: &str, title: &str, catalog: (String, Option<String>), storefront: Option<&str>) -> String {
    let (id, url) = catalog;
    match url {
        Some(u) => {
            put(kind, library_id, Known::Url(u.clone()));
            u
        }
        None => {
            put(kind, library_id, Known::Catalog(id.clone()));
            page(storefront, kind, title, &id)
        }
    }
}

// ---- Where the link goes ----
//
// On macOS Share is a submenu as in Apple Music: Copy Link, then the system's sharing services for
// a URL (AirDrop, Messages, Mail, Notes, Reminders, Freeform, extensions...) with their own titles
// and icons (`share_mac.rs`). Elsewhere there is no system share menu, so the menus have a flat
// Copy Link in Share's place.

/// What to do with a link once it is known.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Via {
    /// The clipboard, with a "Link copied" toast.
    Copy,
    /// The system sharing service at this index of `services()` (macOS).
    Service(usize),
    /// The native NSSharingServicePicker (macOS), when the system's list is empty or can't be read.
    Picker,
}

/// A system sharing service as the Share submenu lists it.
#[derive(Clone, Debug)]
pub struct Service {
    /// Its title as macOS gives it ("AirDrop", "Messages", ...).
    pub title: String,
    /// Its icon as RGBA, ready for Rune (rendered once, when the list is read).
    pub icon: Option<rune::Pixels>,
}

thread_local! {
    /// Scenes and tests: the menu style (`Some(false)`: the flat Copy Link of Linux) and the services listed.
    static STYLE: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
    static FIXTURE: std::cell::RefCell<Option<std::rc::Rc<[Service]>>> = const { std::cell::RefCell::new(None) };
    /// Tests: what was handed to a sharing service instead of performing it.
    #[cfg(test)]
    static PERFORMED: std::cell::RefCell<Vec<(usize, String)>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Share is a submenu with the system services (macOS), else a flat Copy Link.
pub fn submenu() -> bool {
    STYLE.with(|s| s.get()).unwrap_or(cfg!(target_os = "macos"))
}

/// Share is the system's own picker instead of a submenu (macOS): the list of services is empty
/// (or the deprecated API stopped working), so the menus have Copy Link and a Share item that
/// opens `NSSharingServicePicker` at the item. Call `prepare` first (the menus do when built).
pub fn fallback() -> bool {
    submenu() && services().is_empty()
}

/// Scenes and tests: forces the submenu (true) or the flat item (false) on any system.
pub fn set_submenu(on: Option<bool>) {
    STYLE.with(|s| s.set(on));
}

/// Scenes and tests: the services the submenu lists, instead of the system's.
pub fn set_services(list: Option<Vec<Service>>) {
    FIXTURE.with(|f| *f.borrow_mut() = list.map(Into::into));
}

/// A stand-in list for tests and for snaps where the system's can't be read.
pub fn sample_services() -> Vec<Service> {
    ["AirDrop", "Mail", "Messages", "Notes", "Reminders", "Freeform"]
        .into_iter()
        .map(|t| Service { title: t.into(), icon: None })
        .collect()
}

/// Reads the system's services if not read yet (macOS; a few hundred ms the first time, so the
/// app warms it after start). Call it where a menu opens (a handler), never from the view.
pub fn prepare() {
    #[cfg(all(target_os = "macos", not(test)))]
    if FIXTURE.with(|f| f.borrow().is_none()) {
        crate::share_mac::load();
    }
}

/// The services read so far (none before `prepare`, none outside macOS). Cheap: for the view.
pub fn services() -> std::rc::Rc<[Service]> {
    if let Some(list) = FIXTURE.with(|f| f.borrow().clone()) {
        return list;
    }
    #[cfg(all(target_os = "macos", not(test)))]
    if let Some(list) = crate::share_mac::loaded() {
        return list;
    }
    std::rc::Rc::from(Vec::new())
}

/// Hands `url` to service `index` of `services()`; `at` is the point the menu opened at (window
/// points), where macOS anchors the service's sheet. Tests only record it.
pub fn perform(index: usize, url: &str, at: [f32; 2]) {
    #[cfg(test)]
    {
        let _ = at;
        PERFORMED.with(|p| p.borrow_mut().push((index, url.to_string())));
    }
    #[cfg(all(target_os = "macos", not(test)))]
    {
        let url = url.to_string();
        // After this frame: the menu closes first, and the service's sheet comes up over the window.
        dispatch2::DispatchQueue::main().exec_async(move || crate::share_mac::perform(index, &url, at));
    }
    #[cfg(not(any(test, target_os = "macos")))]
    let _ = (index, url, at);
}

/// Opens the native picker for `url` at `at`, the top left of the Share item (window points).
pub fn perform_picker(url: &str, at: [f32; 2]) {
    #[cfg(test)]
    PERFORMED.with(|p| p.borrow_mut().push((usize::MAX, format!("{url} @ {at:?}"))));
    #[cfg(all(target_os = "macos", not(test)))]
    {
        let url = url.to_string();
        dispatch2::DispatchQueue::main().exec_async(move || crate::share_mac::picker(&url, at));
    }
    #[cfg(not(any(test, target_os = "macos")))]
    let _ = (url, at);
}

/// Tests: the (service, url) pairs handed over so far, emptied.
#[cfg(test)]
pub fn take_performed() -> Vec<(usize, String)> {
    PERFORMED.with(|p| std::mem::take(&mut *p.borrow_mut()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn song(id: &str, catalog: Option<&str>, album_id: Option<&str>) -> Song {
        Song {
            id: id.into(),
            catalog_id: catalog.map(String::from),
            title: "Get Lucky (feat. Pharrell)".into(),
            artist: String::new(),
            album: "Random Access Memories".into(),
            album_is_fallback: false,
            duration: 0.0,
            track_number: None,
            disc_number: None,
            explicit: false,
            artwork: None,
            playable: true,
            genre: None,
            year: None,
            album_id: album_id.map(String::from),
            artists: Vec::new(),
        }
    }

    fn coll(kind: CollectionKind, id: &str, title: &str) -> Collection {
        Collection {
            kind,
            id: id.into(),
            title: title.into(),
            subtitle: String::new(),
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

    fn url(l: Link) -> String {
        match l {
            Link::Url(u) => u,
            other => panic!("expected a url, got {other:?}"),
        }
    }

    #[test]
    fn slugs_follow_apples() {
        assert_eq!(slug("Random Access Memories"), "random-access-memories");
        assert_eq!(slug("  Get Lucky (feat. Pharrell) "), "get-lucky-feat-pharrell");
        assert_eq!(slug("♫♫"), "_");
        assert_eq!(slug(""), "_");
    }

    #[test]
    fn a_song_is_its_album_page_with_the_song_id() {
        let s = song("i.abc", Some("617154366"), Some("617154241"));
        assert_eq!(
            url(song_link(&s, Some("it"))),
            "https://music.apple.com/it/album/random-access-memories/617154241?i=617154366"
        );
        // Without an album id: Apple's /song/ page, which redirects to the album.
        let s = song("617154366", None, None);
        assert_eq!(url(song_link(&s, None)), "https://music.apple.com/us/song/get-lucky-feat-pharrell/617154366");
        // A local-style album id is not a catalog one.
        let s = song("617154366", None, Some("local:album"));
        assert!(url(song_link(&s, Some("it"))).contains("/song/"));
    }

    #[test]
    fn songs_without_a_catalog_twin_have_no_link() {
        assert_eq!(song_link(&song("i.abc", None, None), Some("it")), Link::Hidden);
        assert_eq!(song_link(&song("file:///m/a.flac", Some("5"), None), Some("it")), Link::Hidden);
    }

    #[test]
    fn albums_and_playlists_are_built_for_the_storefront() {
        let a = coll(CollectionKind::Album, "617154241", "Random Access Memories");
        assert_eq!(url(collection_link(&a, Some("it"))), "https://music.apple.com/it/album/random-access-memories/617154241");
        let p = coll(CollectionKind::Playlist, "pl.f4d106fed2bd41149aaacabb233eb5eb", "Today's Hits");
        assert_eq!(
            url(collection_link(&p, Some("de"))),
            "https://music.apple.com/de/playlist/today-s-hits/pl.f4d106fed2bd41149aaacabb233eb5eb"
        );
    }

    #[test]
    fn library_albums_are_looked_up_and_other_library_items_hidden() {
        let a = coll(CollectionKind::Album, "l.zzz1", "Nevermind");
        assert_eq!(collection_link(&a, None), Link::Lookup { kind: Kind::Album, id: "l.zzz1".into(), title: "Nevermind".into() });
        assert_eq!(lookup_path(Kind::Album, "l.zzz1").as_deref(), Some("/v1/me/library/albums/l.zzz1/catalog"));
        // A library playlist with no global id (the user's own, never shared) has no page.
        let p = coll(CollectionKind::Playlist, "p.nogloba1", "Mine");
        assert_eq!(collection_link(&p, None), Link::Hidden);
        let mut fav = coll(CollectionKind::Playlist, "p.fav", "Favourite Songs");
        fav.favorites = true;
        assert_eq!(collection_link(&fav, None), Link::Hidden);
    }

    #[test]
    fn a_library_playlist_uses_its_global_id_unless_it_is_private_and_the_users() {
        note(
            Kind::Playlist,
            &json!({"id": "p.glob0001", "attributes": {"playParams": {"id": "p.glob0001", "globalId": "pl.u-abc123"}}}),
        );
        let mut p = coll(CollectionKind::Playlist, "p.glob0001", "Road Trip");
        p.editable = true;
        p.public = Some(true);
        assert_eq!(url(collection_link(&p, Some("it"))), "https://music.apple.com/it/playlist/road-trip/pl.u-abc123");
        p.public = Some(false);
        assert_eq!(collection_link(&p, Some("it")), Link::Hidden, "private and editable: not for others");
        // Added from the catalog: not editable, so its page is Apple's.
        p.editable = false;
        assert_eq!(url(collection_link(&p, Some("it"))), "https://music.apple.com/it/playlist/road-trip/pl.u-abc123");
    }

    #[test]
    fn the_apis_own_url_wins_over_a_built_one() {
        note(
            Kind::Album,
            &json!({"id": "5551", "attributes": {"url": "https://music.apple.com/it/album/the-real-slug/5551"}}),
        );
        let a = coll(CollectionKind::Album, "5551", "Another Title");
        assert_eq!(url(collection_link(&a, Some("us"))), "https://music.apple.com/it/album/the-real-slug/5551");
        // Only music.apple.com links are trusted.
        note(Kind::Album, &json!({"id": "5552", "attributes": {"url": "http://evil.example/x"}}));
        let b = coll(CollectionKind::Album, "5552", "B");
        assert_eq!(url(collection_link(&b, Some("us"))), "https://music.apple.com/us/album/b/5552");
        // Songs: Apple's url already carries ?i=.
        note(
            Kind::Song,
            &json!({"id": "7771", "attributes": {"url": "https://music.apple.com/us/album/x/7770?i=7771"}}),
        );
        assert_eq!(url(song_link(&song("7771", None, Some("7770")), Some("it"))), "https://music.apple.com/us/album/x/7770?i=7771");
    }

    #[test]
    fn artists_by_catalog_id_library_artists_looked_up_local_hidden() {
        let a = Artist { id: "5468295".into(), name: "Daft Punk".into() };
        assert_eq!(url(artist_link(&a, Some("it"))), "https://music.apple.com/it/artist/daft-punk/5468295");
        let l = Artist { id: "r.abc1".into(), name: "Nirvana".into() };
        assert_eq!(artist_link(&l, None), Link::Lookup { kind: Kind::Artist, id: "r.abc1".into(), title: "Nirvana".into() });
        assert_eq!(lookup_path(Kind::Artist, "r.abc1").as_deref(), Some("/v1/me/library/artists/r.abc1/catalog"));
        let f = Artist { id: crate::local::view::artist_id("X"), name: "X".into() };
        assert_eq!(artist_link(&f, None), Link::Hidden);
    }

    #[test]
    fn stations_apple_has_pages_for_the_personal_one_has_none() {
        assert_eq!(
            url(link_of_station("ra.978194965", "Apple Music 1", Some("it"))),
            "https://music.apple.com/it/station/apple-music-1/ra.978194965"
        );
        assert_eq!(link_of_station("ra.u-abc", "Thomas's Station", None), Link::Hidden);
        assert_eq!(link_of_station("something", "x", None), Link::Hidden);
    }

    #[test]
    fn a_lookup_answer_makes_the_link_and_is_remembered() {
        let v = json!({"data": [{"id": "1440857781", "attributes": {}}]});
        let found = parse_lookup(&v).unwrap();
        assert_eq!(found, ("1440857781".to_string(), None));
        let u = resolved(Kind::Album, "l.look1", "Nevermind", found, Some("it"));
        assert_eq!(u, "https://music.apple.com/it/album/nevermind/1440857781");
        // The next click needs no request.
        let a = coll(CollectionKind::Album, "l.look1", "Nevermind");
        assert_eq!(url(collection_link(&a, Some("it"))), u);
        assert_eq!(parse_lookup(&json!({"data": []})), None);
        let with_url = json!({"data": [{"id": "9", "attributes": {"url": "https://music.apple.com/it/album/n/9"}}]});
        assert_eq!(parse_lookup(&with_url).unwrap().1.as_deref(), Some("https://music.apple.com/it/album/n/9"));
    }
}
