//! Apple Music API resources (catalog and library) parsed into plain structs, plus the API
//! paths the pages use. Parsing is lenient: missing fields become None/empty, never errors.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Artwork URL template from the API, e.g. `.../{w}x{h}bb.jpg`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Artwork {
    pub template: String,
    /// Apple's dominant colour for the image (hex, no '#'), catalog artwork only.
    #[serde(default)]
    pub bg: Option<String>,
}

impl Artwork {
    fn parse(v: &Value) -> Option<Self> {
        v["url"].as_str().filter(|s| !s.is_empty()).map(|s| Self {
            template: s.to_string(),
            bg: v["bgColor"].as_str().map(String::from),
        })
    }

    /// URL for a square image `px` pixels wide.
    pub fn url(&self, px: u32) -> String {
        let px = px.to_string();
        // Editorial pictures have a crop code `{c}`; "bb" keeps the whole picture in the box.
        self.template.replace("{w}", &px).replace("{h}", &px).replace("{c}", "bb").replace("{f}", "jpg")
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Song {
    /// Catalog id ("1440857781") or library id ("i.abc…").
    pub id: String,
    /// Catalog id of a library song, when Apple matched it to the catalog.
    pub catalog_id: Option<String>,
    pub title: String,
    pub artist: String,
    pub album: String,
    /// True when `album` is only a stand-in (a local file with no album tag shows its folder's
    /// name). Such a name is no use as a label, e.g. for the initials on a cover tile.
    #[serde(default)]
    pub album_is_fallback: bool,
    /// Seconds.
    pub duration: f64,
    pub track_number: Option<u32>,
    pub disc_number: Option<u32>,
    pub explicit: bool,
    pub artwork: Option<Artwork>,
    /// False when Apple sent no playParams: the song can't be streamed (e.g. pulled from the
    /// catalog) and must not be queued.
    pub playable: bool,
    /// First genre that is not the generic "Music".
    pub genre: Option<String>,
    /// Release year (first 4 characters of the release date).
    pub year: Option<String>,
    /// Catalog album id, from the song's web URL (catalog songs). Library songs look it up.
    #[serde(default)]
    pub album_id: Option<String>,
    /// The song's artists with their ids, when Apple sent `relationships.artists`. Empty
    /// otherwise (the name then stays plain text).
    #[serde(default)]
    pub artists: Vec<Artist>,
}

impl Song {
    /// The name a cover tile takes its initials from: the album, unless that is a stand-in or
    /// empty, then the song's title.
    pub fn tile_name(&self) -> &str {
        if self.album_is_fallback || self.album.trim().is_empty() { &self.title } else { &self.album }
    }

    pub fn parse(v: &Value) -> Option<Self> {
        let a = &v["attributes"];
        Some(Self {
            id: v["id"].as_str()?.to_string(),
            catalog_id: a["playParams"]["catalogId"].as_str().map(String::from),
            title: str_of(&a["name"]),
            artist: str_of(&a["artistName"]),
            album: str_of(&a["albumName"]),
            album_is_fallback: false,
            duration: a["durationInMillis"].as_f64().unwrap_or(0.0) / 1000.0,
            track_number: a["trackNumber"].as_u64().map(|n| n as u32),
            disc_number: a["discNumber"].as_u64().map(|n| n as u32),
            explicit: a["contentRating"].as_str() == Some("explicit"),
            artwork: Artwork::parse(&a["artwork"]),
            playable: a["playParams"].is_object(),
            genre: a["genreNames"].as_array().and_then(|g| g.iter().filter_map(Value::as_str).find(|g| *g != "Music")).map(String::from),
            year: a["releaseDate"].as_str().map(|d| d.chars().take(4).collect::<String>()).filter(|y| !y.is_empty()),
            album_id: a["url"].as_str().and_then(album_id_from_url),
            artists: Artist::list(v),
        })
    }

    /// The simplified item the bridge sends in `nowPlaying` events.
    pub fn from_bridge_item(v: &Value) -> Option<Self> {
        Some(Self {
            id: v["id"].as_str()?.to_string(),
            catalog_id: v["catalogId"].as_str().map(String::from),
            title: str_of(&v["title"]),
            artist: str_of(&v["artist"]),
            album: str_of(&v["album"]),
            album_is_fallback: false,
            duration: v["duration"].as_f64().unwrap_or(0.0),
            track_number: None,
            disc_number: None,
            explicit: false,
            artwork: v["artwork"]
                .as_str()
                .map(|s| Artwork { template: s.to_string(), bg: v["artworkBg"].as_str().map(String::from) }),
            playable: true,
            genre: None,
            year: None,
            album_id: v["albumId"].as_str().map(String::from),
            artists: Vec::new(),
        })
    }

    /// Same recording, whether either side is a library or a catalog song.
    pub fn same_track(&self, other: &Song) -> bool {
        let ids = |s: &Song| [Some(s.id.clone()), s.catalog_id.clone()];
        ids(self).iter().flatten().any(|a| ids(other).iter().flatten().any(|b| a == b))
    }

    /// Id to queue: the catalog id when known (plays more reliably), else our own id.
    pub fn play_id(&self) -> &str {
        self.catalog_id.as_deref().unwrap_or(&self.id)
    }

    /// A local file: its id is `file://` + the absolute path. Plays in-process
    /// (`local::player`), never through the bridge.
    pub fn is_local(&self) -> bool {
        self.id.starts_with(LOCAL_PREFIX)
    }

    /// The file of a local song.
    pub fn local_path(&self) -> Option<&std::path::Path> {
        self.id.strip_prefix(LOCAL_PREFIX).map(std::path::Path::new)
    }

    /// The catalog id, for links to music.apple.com: known for catalog songs and for library
    /// songs Apple matched. None for local files and unmatched library songs.
    pub fn catalog_ref(&self) -> Option<&str> {
        if self.is_local() {
            return None;
        }
        self.catalog_id.as_deref().or((!self.id.starts_with("i.")).then_some(self.id.as_str()))
    }

    /// The id of the local song at `path` (absolute).
    pub fn local_id(path: &std::path::Path) -> String {
        format!("{LOCAL_PREFIX}{}", path.display())
    }
}

/// Id prefix of local songs (no percent-encoding: the rest is the path as is).
pub const LOCAL_PREFIX: &str = "file://";

/// An album or a playlist: anything with a cover, a title line and a track list.
#[derive(Clone, Debug, PartialEq, Eq, Copy, Serialize, Deserialize)]
pub enum CollectionKind {
    Album,
    Playlist,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Collection {
    pub kind: CollectionKind,
    /// Catalog ("1440857781", "pl.…") or library ("l.…", "p.…") id.
    pub id: String,
    pub title: String,
    /// Artist for albums, curator for playlists.
    pub subtitle: String,
    pub artwork: Option<Artwork>,
    /// Release year for albums.
    pub year: Option<String>,
    pub genre: Option<String>,
    pub description: Option<String>,
    pub track_count: Option<u32>,
    /// Library playlists: `lastModifiedDate`, to tell whether a cached copy is still current.
    #[serde(default)]
    pub modified: Option<String>,
    /// A library playlist the user can add songs to.
    #[serde(default)]
    pub editable: bool,
    /// Apple's Favourite Songs playlist (the one library playlist that can't be edited or deleted).
    #[serde(default)]
    pub favorites: bool,
    /// A library playlist shown on the user's profile and in search (`isPublic`), when Apple
    /// says; edits send it back unchanged.
    #[serde(default)]
    pub public: Option<bool>,
    /// Albums: the artists with their ids (library ids "r.…" for library albums, catalog ids
    /// otherwise), when Apple sent `relationships.artists`. Empty for playlists.
    #[serde(default)]
    pub artists: Vec<Artist>,
    /// Filled on detail pages only.
    pub tracks: Vec<Song>,
}

/// An album's artists. Catalog albums send only the ids unless the request says
/// `include=artists`; a lone artist without a name takes the album's artist name.
fn album_artists(v: &Value, artist_name: &str) -> Vec<Artist> {
    let mut artists = Artist::list(v);
    if let [one] = artists.as_mut_slice() {
        if one.name.is_empty() {
            one.name = artist_name.to_string();
        }
    }
    artists
}

impl Collection {
    /// An album without its artists' ids and names: saved before they were kept, or asked
    /// for without `include=artists`. Its artist name can't link until Apple is asked again.
    pub fn lacks_artists(&self) -> bool {
        self.kind == CollectionKind::Album && (self.artists.is_empty() || self.artists.iter().any(|a| a.name.is_empty()))
    }

    pub fn parse(v: &Value) -> Option<Self> {
        let kind = match v["type"].as_str()? {
            "albums" | "library-albums" => CollectionKind::Album,
            "playlists" | "library-playlists" => CollectionKind::Playlist,
            _ => return None,
        };
        let a = &v["attributes"];
        let subtitle = match kind {
            CollectionKind::Album => str_of(&a["artistName"]),
            CollectionKind::Playlist => a["curatorName"].as_str().unwrap_or("Playlist").to_string(),
        };
        let description = a["description"]["standard"]
            .as_str()
            .or_else(|| a["description"]["short"].as_str())
            .or_else(|| a["editorialNotes"]["short"].as_str())
            .map(strip_tags);
        Some(Self {
            kind,
            id: v["id"].as_str()?.to_string(),
            title: str_of(&a["name"]),
            subtitle,
            artwork: Artwork::parse(&a["artwork"]),
            year: a["releaseDate"].as_str().and_then(|d| d.get(..4)).map(String::from),
            genre: a["genreNames"][0].as_str().filter(|g| *g != "Music").map(String::from),
            description,
            track_count: a["trackCount"].as_u64().map(|n| n as u32),
            modified: a["lastModifiedDate"].as_str().map(String::from),
            editable: a["canEdit"].as_bool() == Some(true),
            favorites: v["type"].as_str() == Some("library-playlists")
                && a["canEdit"].as_bool() == Some(false)
                && a["canDelete"].as_bool() == Some(false),
            public: a["isPublic"].as_bool(),
            artists: if kind == CollectionKind::Album { album_artists(v, &str_of(&a["artistName"])) } else { Vec::new() },
            tracks: v["relationships"]["tracks"]["data"]
                .as_array()
                .map(|ts| ts.iter().filter_map(Song::parse).collect())
                .unwrap_or_default(),
        })
    }
}

/// One page of a paginated list, with the API's `next` path when there is more.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next: Option<String>,
    /// The whole list's length (`meta.total`), when Apple sends it (library lists do).
    pub total: Option<u64>,
}

impl<T> Page<T> {
    pub fn parse(v: &Value, item: impl Fn(&Value) -> Option<T>) -> Self {
        Self {
            items: v["data"].as_array().map(|d| d.iter().filter_map(&item).collect()).unwrap_or_default(),
            next: v["next"].as_str().map(String::from),
            total: v["meta"]["total"].as_u64(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SearchResults {
    pub songs: Vec<Song>,
    pub albums: Vec<Collection>,
    pub playlists: Vec<Collection>,
}

impl SearchResults {
    pub fn parse(v: &Value) -> Self {
        let r = &v["results"];
        let list = |k: &str| r[k]["data"].as_array().cloned().unwrap_or_default();
        Self {
            songs: list("songs").iter().filter_map(Song::parse).collect(),
            albums: list("albums").iter().filter_map(Collection::parse).collect(),
            playlists: list("playlists").iter().filter_map(Collection::parse).collect(),
        }
    }
}

/// A titled row: albums/playlists (cards), plus songs and radio stations on browse pages.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Shelf {
    pub title: String,
    pub items: Vec<Collection>,
    #[serde(default)]
    pub songs: Vec<Song>,
    #[serde(default)]
    pub stations: Vec<Station>,
}

impl Shelf {
    /// Sorts mixed API resources into cards, songs and stations. None when nothing is usable.
    pub fn from_resources(title: &str, resources: &[Value]) -> Option<Self> {
        let shelf = Self {
            title: title.to_string(),
            items: resources.iter().filter_map(Collection::parse).collect(),
            songs: resources.iter().filter_map(|r| (r["type"] == "songs").then(|| Song::parse(r)).flatten()).collect(),
            stations: resources.iter().filter_map(Station::parse).collect(),
        };
        (!shelf.is_empty()).then_some(shelf)
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty() && self.songs.is_empty() && self.stations.is_empty()
    }
}

/// A radio station (live like Apple Music 1, or a personal / artist station).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Station {
    pub id: String,
    pub name: String,
    pub artwork: Option<Artwork>,
    pub live: bool,
}

impl Station {
    pub fn parse(v: &Value) -> Option<Self> {
        if v["type"].as_str()? != "stations" {
            return None;
        }
        let a = &v["attributes"];
        Some(Self {
            id: v["id"].as_str()?.to_string(),
            name: str_of(&a["name"]),
            artwork: Artwork::parse(&a["artwork"]),
            live: a["isLive"].as_bool() == Some(true),
        })
    }
}

/// A library artist (Apple sends only the name; no artwork for library artists).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Artist {
    pub id: String,
    pub name: String,
}

impl Artist {
    pub fn parse(v: &Value) -> Option<Self> {
        Some(Self { id: v["id"].as_str()?.to_string(), name: str_of(&v["attributes"]["name"]) })
    }

    /// The artists of a resource's `relationships.artists` (none when Apple sent no data).
    pub fn list(v: &Value) -> Vec<Self> {
        v["relationships"]["artists"]["data"].as_array().map(|a| a.iter().filter_map(Self::parse).collect()).unwrap_or_default()
    }

    /// A library artist id ("r.…"), as opposed to a catalog one ("5468295").
    pub fn is_library_id(id: &str) -> bool {
        id.starts_with("r.") || id.starts_with("l.")
    }
}

/// Browse ("New"): the editorial grouping's shelves.
pub fn parse_editorial(v: &Value) -> Vec<Shelf> {
    let kids = &v["data"][0]["relationships"]["tabs"]["data"][0]["relationships"]["children"]["data"];
    kids.as_array()
        .map(|ks| {
            ks.iter()
                .filter_map(|k| {
                    let title = k["attributes"]["name"].as_str()?;
                    Shelf::from_resources(title, k["relationships"]["contents"]["data"].as_array()?)
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Made for You: the personal recommendation groups.
pub fn parse_recommendations(v: &Value) -> Vec<Shelf> {
    v["data"]
        .as_array()
        .map(|gs| {
            gs.iter()
                .filter_map(|g| {
                    let title = g["attributes"]["title"]["stringForDisplay"].as_str()?;
                    Shelf::from_resources(title, g["relationships"]["contents"]["data"].as_array()?)
                })
                .collect()
        })
        .unwrap_or_default()
}

/// `https://music.apple.com/it/album/numb/590423275?i=590423552` → `590423275`.
fn album_id_from_url(url: &str) -> Option<String> {
    let path = url.split('?').next()?;
    let mut parts = path.split('/');
    parts.position(|p| p == "album")?;
    let id = parts.last()?;
    (!id.is_empty() && id.chars().all(|c| c.is_ascii_digit())).then(|| id.to_string())
}

/// API requests: (path, params). Paths may use `{{storefrontId}}`.
pub mod api {
    use super::*;

    /// A read about the signed-in account: its library, history, ratings, recommendations or
    /// station. Its reply is dropped if the account changed meanwhile (`state::Msg::Account`).
    pub fn is_personal(path: &str, params: &Value) -> bool {
        path.starts_with("/v1/me/") || params["filter[identity]"] == "personal"
    }

    pub fn search(term: &str) -> (String, Value) {
        (
            "/v1/catalog/{{storefrontId}}/search".into(),
            json!({"term": term, "types": "songs,albums,playlists", "limit": 20}),
        )
    }

    pub fn album(id: &str) -> (String, Value) {
        if id.starts_with("l.") {
            (format!("/v1/me/library/albums/{id}"), json!({"include": "tracks,artists"}))
        } else {
            // Catalog albums list their artists' ids anyway, but their names only with
            // `include=artists` (checked live 2026-11-09); the tracks still come.
            (format!("/v1/catalog/{{{{storefrontId}}}}/albums/{id}"), json!({"include": "artists", "extend": "editorialVideo"}))
        }
    }

    /// Only a catalog album's motion art (`editorialVideo`, about 17 KB with the track ids):
    /// for the playing song's album when its page wasn't loaded.
    pub fn album_motion(id: &str) -> (String, Value) {
        (format!("/v1/catalog/{{{{storefrontId}}}}/albums/{id}"), json!({"extend": "editorialVideo", "fields[albums]": "editorialVideo"}))
    }

    pub fn playlist(id: &str) -> (String, Value) {
        if id.starts_with("p.") {
            (format!("/v1/me/library/playlists/{id}"), json!({"include": "tracks"}))
        } else {
            (format!("/v1/catalog/{{{{storefrontId}}}}/playlists/{id}"), json!({}))
        }
    }

    pub fn library_albums() -> (String, Value) {
        ("/v1/me/library/albums".into(), json!({"limit": 100, "sort": "-dateAdded"}))
    }

    pub fn library_playlists() -> (String, Value) {
        ("/v1/me/library/playlists".into(), json!({"limit": 100}))
    }

    /// Oldest added first: the API sorts by date added (but never sends the dates), and the
    /// `next` links keep the sort, so list order = date-added order.
    pub fn library_songs() -> (String, Value) {
        ("/v1/me/library/songs".into(), json!({"limit": 100, "sort": "dateAdded"}))
    }

    /// The 10 newest songs/albums: with `meta.total`, a cheap "did the library change?" probe.
    pub fn newest(kind: &str) -> (String, Value) {
        (format!("/v1/me/library/{kind}"), json!({"limit": 10, "sort": "-dateAdded"}))
    }

    pub fn recently_played() -> (String, Value) {
        ("/v1/me/recent/played".into(), json!({"limit": 10}))
    }

    pub fn recently_added() -> (String, Value) {
        ("/v1/me/library/recently-added".into(), json!({"limit": 25}))
    }

    /// Browse ("New"): Apple's editorial shelves, with their contents.
    pub fn editorial() -> (String, Value) {
        (
            "/v1/editorial/{{storefrontId}}/groupings".into(),
            json!({"platform": "web", "name": "music", "include[editorial-elements]": "contents"}),
        )
    }

    /// Made for You.
    pub fn recommendations() -> (String, Value) {
        ("/v1/me/recommendations".into(), json!({"limit": 20}))
    }

    /// Apple's live radio (Apple Music 1, Hits, …).
    pub fn live_radio() -> (String, Value) {
        ("/v1/catalog/{{storefrontId}}/stations".into(), json!({"filter[featured]": "apple-music-live-radio"}))
    }

    /// The user's personal station.
    pub fn personal_station() -> (String, Value) {
        ("/v1/catalog/{{storefrontId}}/stations".into(), json!({"filter[identity]": "personal"}))
    }

    pub fn library_artists() -> (String, Value) {
        ("/v1/me/library/artists".into(), json!({"limit": 100}))
    }

    /// A library artist's albums in the library, or (a catalog id, from an album's or song's
    /// artist link) the artist's catalog albums.
    pub fn artist_albums(id: &str) -> (String, Value) {
        if Artist::is_library_id(id) {
            (format!("/v1/me/library/artists/{id}/albums"), json!({"limit": 100}))
        } else {
            (format!("/v1/catalog/{{{{storefrontId}}}}/artists/{id}/albums"), json!({"limit": 100}))
        }
    }

    /// The library album a library song belongs to (Go to Album).
    pub fn library_song_album(id: &str) -> (String, Value) {
        (format!("/v1/me/library/songs/{id}/albums"), json!({}))
    }

    /// Follow a page's `next` path. It carries its own offset but not the page size, which
    /// would fall back to 25; library endpoints allow up to 100.
    pub fn next(next: &str, limit: u32) -> (String, Value) {
        (next.to_string(), json!({"limit": limit}))
    }
}

fn str_of(v: &Value) -> String {
    v.as_str().unwrap_or("").to_string()
}

/// Editorial notes come with HTML tags (<b>, <br>); keep the text.
pub(crate) fn strip_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tile_name_is_the_album_unless_that_is_a_stand_in() {
        let mut song = Song { title: "Powerade".into(), album: "Music".into(), album_is_fallback: true, ..Song::parse(&serde_json::json!({"id": "1"})).unwrap() };
        assert_eq!(song.tile_name(), "Powerade", "a folder-name album gives the title");
        song.album_is_fallback = false;
        assert_eq!(song.tile_name(), "Music", "a real album called Music keeps its name");
        song.album = "  ".into();
        assert_eq!(song.tile_name(), "Powerade", "an empty album gives the title");
    }

    #[test]
    fn parses_a_catalog_album_with_tracks() {
        let v = json!({"id": "1", "type": "albums", "attributes": {
            "name": "Discovery", "artistName": "Daft Punk", "releaseDate": "2001-03-07",
            "genreNames": ["Electronic", "Music"], "trackCount": 14,
            "artwork": {"url": "https://x/{w}x{h}bb.jpg"}},
            "relationships": {"tracks": {"data": [{"id": "697195462", "type": "songs", "attributes": {
                "name": "One More Time", "artistName": "Daft Punk", "albumName": "Discovery",
                "durationInMillis": 320357, "trackNumber": 1, "contentRating": "explicit"}}]}}});
        let a = Collection::parse(&v).unwrap();
        assert_eq!(a.kind, CollectionKind::Album);
        assert_eq!(a.year.as_deref(), Some("2001"));
        assert_eq!(a.genre.as_deref(), Some("Electronic"));
        assert_eq!(a.artwork.unwrap().url(300), "https://x/300x300bb.jpg");
        assert_eq!(a.tracks.len(), 1);
        assert!(a.tracks[0].explicit);
        assert!((a.tracks[0].duration - 320.357).abs() < 1e-9);
    }

    #[test]
    fn albums_and_songs_keep_their_artist_ids() {
        let artists = json!({"data": [
            {"id": "r.1", "type": "library-artists", "attributes": {"name": "A"}},
            {"id": "r.2", "type": "library-artists", "attributes": {"name": "B"}}]});
        let v = json!({"id": "l.x", "type": "library-albums", "attributes": {"name": "T", "artistName": "A & B"},
            "relationships": {"artists": artists.clone(), "tracks": {"data": [
                {"id": "i.1", "type": "library-songs", "attributes": {"name": "S"}, "relationships": {"artists": artists}}]}}});
        let c = Collection::parse(&v).unwrap();
        let ids: Vec<_> = c.artists.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(ids, ["r.1", "r.2"]);
        assert_eq!(c.artists[1].name, "B");
        assert_eq!(c.tracks[0].artists.len(), 2);
        // A playlist never links its curator; a resource without the relationship has none.
        let pl = json!({"id": "p.1", "type": "library-playlists", "relationships": {"artists": {"data": [{"id": "r.9"}]}}});
        assert!(Collection::parse(&pl).unwrap().artists.is_empty());
        let bare = json!({"id": "1", "type": "albums", "attributes": {"name": "T"}});
        assert!(Collection::parse(&bare).unwrap().artists.is_empty());
    }

    #[test]
    fn old_cache_files_without_artists_still_load() {
        let mut c = serde_json::to_value(Collection::parse(&json!({"id": "1", "type": "albums", "attributes": {"name": "T"},
            "relationships": {"tracks": {"data": [{"id": "9", "type": "songs", "attributes": {"name": "S"}}]}}})).unwrap()).unwrap();
        c.as_object_mut().unwrap().remove("artists");
        c["tracks"][0].as_object_mut().unwrap().remove("artists");
        let back: Collection = serde_json::from_value(c).unwrap();
        assert!(back.artists.is_empty() && back.tracks[0].artists.is_empty());
    }

    #[test]
    fn artist_albums_ask_the_library_or_the_catalog_by_id() {
        assert_eq!(api::artist_albums("r.abc").0, "/v1/me/library/artists/r.abc/albums");
        assert_eq!(api::artist_albums("5468295").0, "/v1/catalog/{{storefrontId}}/artists/5468295/albums");
    }

    #[test]
    fn album_id_comes_from_the_song_url() {
        assert_eq!(album_id_from_url("https://music.apple.com/it/album/numb/590423275?i=590423552").as_deref(), Some("590423275"));
        assert_eq!(album_id_from_url("https://music.apple.com/us/playlist/x/pl.123"), None);
    }

    #[test]
    fn favourites_is_the_library_playlist_that_cannot_be_edited_or_deleted() {
        let v = |edit: bool, delete: bool| json!({"id": "p.x", "type": "library-playlists",
            "attributes": {"name": "Brani preferiti", "canEdit": edit, "canDelete": delete}});
        assert!(Collection::parse(&v(false, false)).unwrap().favorites);
        assert!(!Collection::parse(&v(false, true)).unwrap().favorites);
        assert!(Collection::parse(&v(true, true)).unwrap().editable);
    }

    #[test]
    fn editorial_shelves_mix_cards_songs_and_stations() {
        let v = json!({"data": [{"relationships": {"tabs": {"data": [{"relationships": {"children": {"data": [
            {"attributes": {"name": "New"}, "relationships": {"contents": {"data": [
                {"id": "1", "type": "albums", "attributes": {"name": "A"}},
                {"id": "2", "type": "songs", "attributes": {"name": "S", "playParams": {}}},
                {"id": "ra.1", "type": "stations", "attributes": {"name": "R", "isLive": true}}]}}},
            {"attributes": {"name": "Empty"}, "relationships": {"contents": {"data": []}}},
            {"attributes": {}, "relationships": {}}]}}}]}}}]});
        let shelves = parse_editorial(&v);
        assert_eq!(shelves.len(), 1);
        assert_eq!((shelves[0].items.len(), shelves[0].songs.len(), shelves[0].stations.len()), (1, 1, 1));
        assert!(shelves[0].stations[0].live);
    }

    #[test]
    fn library_song_prefers_catalog_id_for_playback() {
        let v = json!({"id": "i.abc", "type": "library-songs",
            "attributes": {"name": "x", "playParams": {"id": "i.abc", "catalogId": "123"}}});
        assert_eq!(Song::parse(&v).unwrap().play_id(), "123");
    }

    #[test]
    fn album_path_substitutes_storefront_placeholder_literally() {
        assert_eq!(api::album("42").0, "/v1/catalog/{{storefrontId}}/albums/42");
        assert_eq!(api::album("l.x").0, "/v1/me/library/albums/l.x");
        let personal = |(path, params): (String, Value)| api::is_personal(&path, &params);
        assert!(personal(api::album("l.x")) && personal(api::library_songs()) && personal(api::personal_station()));
        assert!(!personal(api::album("42")) && !personal(api::live_radio()) && !personal(api::search("x")));
    }

    #[test]
    fn album_requests_ask_for_the_artists_names() {
        assert_eq!(api::album("42").1["include"], "artists");
        assert_eq!(api::album("l.x").1["include"], "tracks,artists");
    }

    #[test]
    fn catalog_album_artists_without_names() {
        // What Apple sends for a catalog album asked without include=artists (2026-11-09).
        let reply = |ids: &[&str]| {
            let data: Vec<_> = ids.iter().map(|id| json!({"id": id, "type": "artists", "href": format!("/v1/catalog/it/artists/{id}")})).collect();
            json!({"id": "6776282087", "type": "albums", "attributes": {"name": "Aches", "artistName": "A & B"},
                "relationships": {"artists": {"data": data}, "tracks": {"data": []}}})
        };
        let one = Collection::parse(&reply(&["1693772033"])).unwrap();
        assert_eq!((one.artists[0].id.as_str(), one.artists[0].name.as_str()), ("1693772033", "A & B"));
        assert!(!one.lacks_artists());
        // Two nameless ids can't be told apart in a menu: the page asks again with the names.
        assert!(Collection::parse(&reply(&["1", "2"])).unwrap().lacks_artists());
        assert!(Collection::parse(&reply(&[])).unwrap().lacks_artists());
        let pl = json!({"id": "p.1", "type": "library-playlists", "attributes": {"name": "P"}});
        assert!(!Collection::parse(&pl).unwrap().lacks_artists(), "playlists never link a curator");
    }
}
