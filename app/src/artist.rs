//! A catalog artist's page, as music.apple.com builds it: one request for the artist with its
//! "views" (latest release, top songs, albums, singles, …), parsed into sections sorted newest
//! first, plus the extra pages a section fetches when it is scrolled to its end or opened in full.
//!
//! Request shape (`api::page`), as Apple's web app asks for it:
//! `GET /v1/catalog/{sf}/artists/{id}?views=…&extend=editorialArtwork,artistBio&limit[artists:top-songs]=20`.
//! The reply is `data[0]` with `views.<name> = { href, next?, attributes: { title }, data: [...] }`.
//! A view's `next` is a path like `/v1/catalog/{sf}/artists/{id}/view/full-albums?offset=10`.

use serde_json::{Value, json};

use crate::models::{Artist, Artwork, Collection, Song};

/// The views asked for, in the request's order.
pub const VIEWS: &str = "latest-release,top-songs,full-albums,singles,live-albums,compilation-albums,appears-on-albums,featured-playlists,similar-artists";

/// Top songs shown at most (Apple's web page shows 20 in a sideways grid).
pub const TOP_SONGS_MAX: usize = 20;

/// The card shelves of the page, in the page's order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum Shelf {
    Albums,
    Singles,
    Live,
    Compilations,
    AppearsOn,
    Playlists,
}

impl Shelf {
    pub const ALL: [Shelf; 6] = [Shelf::Albums, Shelf::Singles, Shelf::Live, Shelf::Compilations, Shelf::AppearsOn, Shelf::Playlists];

    /// Apple's view name.
    pub fn view(self) -> &'static str {
        match self {
            Shelf::Albums => "full-albums",
            Shelf::Singles => "singles",
            Shelf::Live => "live-albums",
            Shelf::Compilations => "compilation-albums",
            Shelf::AppearsOn => "appears-on-albums",
            Shelf::Playlists => "featured-playlists",
        }
    }

    /// The shelf's title (the app is in English; Apple's titles follow the storefront).
    pub fn title(self) -> &'static str {
        match self {
            Shelf::Albums => "Albums",
            Shelf::Singles => "Singles & EPs",
            Shelf::Live => "Live Albums",
            Shelf::Compilations => "Compilations",
            Shelf::AppearsOn => "Appears On",
            Shelf::Playlists => "Playlists",
        }
    }

    /// Album shelves are sorted newest first; playlists keep Apple's order.
    pub fn by_date(self) -> bool {
        self != Shelf::Playlists
    }
}

/// One shelf's items, sorted, with the path of Apple's next page when there is more.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Section {
    pub items: Vec<Collection>,
    /// `releaseDate` of each item (same order as `items`; "" when Apple sent none).
    pub dates: Vec<String>,
    pub next: Option<String>,
    /// A next page is on its way.
    pub loading: bool,
    /// The See All page is open: keep fetching pages until there are none.
    pub all: bool,
    pub error: Option<String>,
}

impl Section {
    /// Adds a page of (date, item), drops ids already here, and sorts newest first when `by_date`.
    pub fn merge(&mut self, page: Vec<(String, Collection)>, by_date: bool) {
        let mut pairs: Vec<(String, Collection)> = std::mem::take(&mut self.dates).into_iter().zip(std::mem::take(&mut self.items)).collect();
        for (d, c) in page {
            if !pairs.iter().any(|(_, x)| x.id == c.id) {
                pairs.push((d, c));
            }
        }
        if by_date {
            sort_newest_first(&mut pairs);
        }
        (self.dates, self.items) = pairs.into_iter().unzip();
    }
}

/// Newest first by `releaseDate` (ISO dates, or just a year, compare as text); undated items
/// go last; equal dates keep Apple's order.
pub fn sort_newest_first(pairs: &mut [(String, Collection)]) {
    pairs.sort_by(|a, b| match (a.0.is_empty(), b.0.is_empty()) {
        (true, true) => std::cmp::Ordering::Equal,
        (true, false) => std::cmp::Ordering::Greater,
        (false, true) => std::cmp::Ordering::Less,
        _ => b.0.cmp(&a.0),
    });
}

/// A similar artist: catalog artists have a picture.
#[derive(Clone, Debug, PartialEq)]
pub struct ArtistCard {
    pub id: String,
    pub name: String,
    pub artwork: Option<Artwork>,
}

impl ArtistCard {
    pub fn artist(&self) -> Artist {
        Artist { id: self.id.clone(), name: self.name.clone() }
    }
}

/// The header's wide picture: a template with only `{w}` left, and its width / height.
#[derive(Clone, Debug, PartialEq)]
pub struct Hero {
    pub art: Artwork,
    pub aspect: f32,
    /// The pixel width to ask for (never more than Apple has).
    pub px: u32,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ArtistPage {
    /// The catalog id (a library artist's page is its catalog artist's).
    pub id: String,
    pub name: String,
    pub artwork: Option<Artwork>,
    pub hero: Option<Hero>,
    pub bio: Option<String>,
    pub latest: Option<Collection>,
    pub latest_date: Option<String>,
    pub top_songs: Vec<Song>,
    pub top_next: Option<String>,
    pub top_loading: bool,
    /// Indexed like `Shelf::ALL`; empty sections are hidden by the view.
    pub sections: [Section; 6],
    pub similar: Vec<ArtistCard>,
}

/// Editorial pictures to try for the header, best first.
const HERO_KINDS: [&str; 5] = ["bannerUber", "centeredFullscreenBackground", "storeFlowcase", "subscriptionHero", "superHeroWide"];

/// The widest the header picture is ever drawn, in pixels (a wide window at 2×).
const HERO_MAX_PX: u32 = 2400;

fn hero_of(a: &Value) -> Option<Hero> {
    let ed = &a["editorialArtwork"];
    HERO_KINDS.iter().find_map(|k| {
        let v = &ed[*k];
        let url = v["url"].as_str().filter(|u| !u.is_empty())?;
        let (w, h) = (v["width"].as_f64().unwrap_or(0.0), v["height"].as_f64().unwrap_or(0.0));
        if w <= 0.0 || h <= 0.0 {
            return None;
        }
        let px = (w as u32).min(HERO_MAX_PX);
        let ph = ((px as f64) * h / w).round() as u32;
        // `{h}` is fixed now, so `Artwork::url(px)` asks for exactly this aspect.
        let template = url.replace("{h}", &ph.to_string());
        Some(Hero {
            art: Artwork { template, bg: v["bgColor"].as_str().map(String::from) },
            aspect: (w / h) as f32,
            px,
        })
    })
}

fn bio_of(a: &Value) -> Option<String> {
    let s = a["artistBio"]
        .as_str()
        .or_else(|| a["artistBio"]["standard"].as_str())
        .or_else(|| a["editorialNotes"]["standard"].as_str())
        .or_else(|| a["editorialNotes"]["short"].as_str())?;
    let s = crate::models::strip_tags(s).split_whitespace().collect::<Vec<_>>().join(" ");
    (!s.is_empty()).then_some(s)
}

/// (releaseDate, album or playlist) for each resource of a view's `data`.
fn dated(data: &Value) -> Vec<(String, Collection)> {
    data.as_array()
        .map(|d| {
            d.iter()
                .filter_map(|v| Some((v["attributes"]["releaseDate"].as_str().unwrap_or("").to_string(), Collection::parse(v)?)))
                .collect()
        })
        .unwrap_or_default()
}

/// One page of a view (the first, inside the artist, or a later one from `next`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ViewPage {
    pub items: Vec<(String, Collection)>,
    pub songs: Vec<Song>,
    pub next: Option<String>,
}

impl ViewPage {
    /// A view object (`{data, next}`), or a page reply of the same shape.
    pub fn parse(v: &Value) -> Self {
        Self {
            items: dated(&v["data"]),
            songs: v["data"].as_array().map(|d| d.iter().filter(|x| x["type"] == "songs").filter_map(Song::parse).collect()).unwrap_or_default(),
            next: v["next"].as_str().map(String::from),
        }
    }
}

impl ArtistPage {
    /// The artist reply (`{data: [artist]}`, or the artist resource itself).
    pub fn parse(v: &Value) -> Option<Self> {
        let r = if v["data"].is_array() { &v["data"][0] } else { v };
        let id = r["id"].as_str()?.to_string();
        let a = &r["attributes"];
        let views = &r["views"];
        let mut page = Self {
            id,
            name: a["name"].as_str().unwrap_or("").to_string(),
            artwork: a["artwork"]["url"].as_str().filter(|u| !u.is_empty()).map(|u| Artwork { template: u.to_string(), bg: a["artwork"]["bgColor"].as_str().map(String::from) }),
            hero: hero_of(a),
            bio: bio_of(a),
            ..Self::default()
        };
        let latest = dated(&views["latest-release"]["data"]).into_iter().next();
        if let Some((d, c)) = latest {
            page.latest_date = (!d.is_empty()).then_some(d);
            page.latest = Some(c);
        }
        let top = ViewPage::parse(&views["top-songs"]);
        page.top_songs = top.songs.into_iter().take(TOP_SONGS_MAX).collect();
        page.top_next = top.next.filter(|_| page.top_songs.len() < TOP_SONGS_MAX);
        for (i, shelf) in Shelf::ALL.iter().enumerate() {
            let vp = ViewPage::parse(&views[shelf.view()]);
            page.sections[i].merge(vp.items, shelf.by_date());
            page.sections[i].next = vp.next;
        }
        page.similar = views["similar-artists"]["data"]
            .as_array()
            .map(|d| {
                d.iter()
                    .filter_map(|x| {
                        let a = &x["attributes"];
                        Some(ArtistCard {
                            id: x["id"].as_str()?.to_string(),
                            name: a["name"].as_str().unwrap_or("").to_string(),
                            artwork: a["artwork"]["url"].as_str().filter(|u| !u.is_empty()).map(|u| Artwork { template: u.to_string(), bg: None }),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        Some(page)
    }

    pub fn section(&self, shelf: Shelf) -> &Section {
        &self.sections[Shelf::ALL.iter().position(|s| *s == shelf).unwrap_or(0)]
    }

    pub fn section_mut(&mut self, shelf: Shelf) -> &mut Section {
        &mut self.sections[Shelf::ALL.iter().position(|s| *s == shelf).unwrap_or(0)]
    }

    /// Nothing at all to show (an artist with no releases in this storefront).
    pub fn is_empty(&self) -> bool {
        self.latest.is_none() && self.top_songs.is_empty() && self.sections.iter().all(|s| s.items.is_empty()) && self.similar.is_empty()
    }
}

/// The catalog id a library artist reply links to (`include=catalog`), if Apple matched it.
pub fn catalog_id_of(v: &Value) -> Option<String> {
    let r = if v["data"].is_array() { &v["data"][0] } else { v };
    r["relationships"]["catalog"]["data"][0]["id"].as_str().filter(|s| !s.is_empty()).map(String::from)
}

/// "2024-10-04" → "OCT 4, 2024"; a bare year stays as it is.
pub fn fmt_release(date: &str) -> String {
    const M: [&str; 12] = ["JAN", "FEB", "MAR", "APR", "MAY", "JUN", "JUL", "AUG", "SEP", "OCT", "NOV", "DEC"];
    let mut parts = date.split('-');
    let (y, m, d) = (parts.next(), parts.next().and_then(|m| m.parse::<usize>().ok()), parts.next().and_then(|d| d.parse::<u32>().ok()));
    match (y, m, d) {
        (Some(y), Some(m @ 1..=12), Some(d)) => format!("{} {d}, {y}", M[m - 1]),
        (Some(y), Some(m @ 1..=12), None) => format!("{} {y}", M[m - 1]),
        (Some(y), _, _) => y.to_string(),
        _ => String::new(),
    }
}

/// Requests: (path, params). Paths may use `{{storefrontId}}`.
pub mod api {
    use super::*;

    /// The whole page. `full`: with the extra parameters Apple's web app sends; a retry after a
    /// failure leaves them out (`limit[artists:top-songs]` is the one not checked live).
    pub fn page(catalog_id: &str, full: bool) -> (String, Value) {
        let mut p = json!({"views": VIEWS, "extend": "editorialArtwork,artistBio"});
        if full {
            p["limit[artists:top-songs]"] = json!(TOP_SONGS_MAX);
        }
        (format!("/v1/catalog/{{{{storefrontId}}}}/artists/{catalog_id}"), p)
    }

    /// A library artist with its catalog match.
    pub fn library_artist(id: &str) -> (String, Value) {
        (format!("/v1/me/library/artists/{id}"), json!({"include": "catalog"}))
    }

    /// The next page of a view. `limit`: None leaves Apple's default page size.
    pub fn more(next: &str, limit: Option<u32>) -> (String, Value) {
        (next.to_string(), limit.map_or_else(|| json!({}), |l| json!({"limit": l})))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn album(id: &str, date: &str) -> Value {
        json!({"id": id, "type": "albums", "attributes": {"name": format!("A{id}"), "artistName": "X", "releaseDate": date}})
    }

    #[test]
    fn views_parse_into_sorted_sections_and_empty_ones_stay_empty() {
        let v = json!({"data": [{"id": "123", "type": "artists",
            "attributes": {"name": "Pendulum", "artistBio": "<b>Drum</b> and\n bass.",
                "artwork": {"url": "https://a/{w}x{h}bb.{f}"},
                "editorialArtwork": {"storeFlowcase": {"url": "https://e/{w}x{h}{c}.{f}", "width": 4320, "height": 1080}}},
            "views": {
                "latest-release": {"data": [album("9", "2025-03-01")]},
                "top-songs": {"next": "/v1/x?offset=10", "data": [
                    {"id": "s1", "type": "songs", "attributes": {"name": "Watercolour", "playParams": {"id": "s1"}}},
                    {"id": "s2", "type": "songs", "attributes": {"name": "Witchcraft", "playParams": {"id": "s2"}}}]},
                "full-albums": {"next": "/v1/catalog/us/artists/123/view/full-albums?offset=3",
                    "data": [album("1", "2005-07-04"), album("2", "2010-05-24"), album("3", "2008-05-12"), album("4", "")]},
                "singles": {"data": []},
                "featured-playlists": {"data": [{"id": "pl.b", "type": "playlists", "attributes": {"name": "B"}},
                    {"id": "pl.a", "type": "playlists", "attributes": {"name": "A"}}]},
                "similar-artists": {"data": [{"id": "77", "type": "artists", "attributes": {"name": "Chase & Status", "artwork": {"url": "https://s/{w}x{h}bb.{f}"}}}]}
            }}]});
        let p = ArtistPage::parse(&v).unwrap();
        assert_eq!(p.name, "Pendulum");
        assert_eq!(p.bio.as_deref(), Some("Drum and bass."));
        assert_eq!(p.latest.as_ref().unwrap().id, "9");
        assert_eq!(p.latest_date.as_deref(), Some("2025-03-01"));
        assert_eq!(p.top_songs.len(), 2);
        assert_eq!(p.top_next.as_deref(), Some("/v1/x?offset=10"));
        let albums = p.section(Shelf::Albums);
        let ids: Vec<_> = albums.items.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, ["2", "3", "1", "4"], "newest first, undated last");
        assert_eq!(albums.next.as_deref(), Some("/v1/catalog/us/artists/123/view/full-albums?offset=3"));
        assert!(p.section(Shelf::Singles).items.is_empty());
        assert!(p.section(Shelf::Live).items.is_empty(), "a view Apple left out");
        let pl: Vec<_> = p.section(Shelf::Playlists).items.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(pl, ["pl.b", "pl.a"], "playlists keep Apple's order");
        assert_eq!(p.similar[0].name, "Chase & Status");
        let hero = p.hero.unwrap();
        assert_eq!(hero.px, 2400);
        assert_eq!(hero.art.url(hero.px), "https://e/2400x600bb.jpg");
        assert!((hero.aspect - 4.0).abs() < 1e-3);
    }

    #[test]
    fn a_later_page_merges_into_the_sorted_section_once() {
        let mut s = Section::default();
        s.merge(dated(&json!([album("1", "2001"), album("2", "2019-01-01")])), true);
        let page = ViewPage::parse(&json!({"data": [album("3", "2010-06-01"), album("2", "2019-01-01")]}));
        assert!(page.next.is_none());
        s.merge(page.items, true);
        let ids: Vec<_> = s.items.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, ["2", "3", "1"]);
        assert_eq!(s.dates, ["2019-01-01", "2010-06-01", "2001"]);
    }

    #[test]
    fn a_library_artist_resolves_to_its_catalog_artist() {
        let v = json!({"data": [{"id": "r.abc", "type": "library-artists", "attributes": {"name": "P"},
            "relationships": {"catalog": {"data": [{"id": "28673423", "type": "artists"}]}}}]});
        assert_eq!(catalog_id_of(&v).as_deref(), Some("28673423"));
        let none = json!({"data": [{"id": "r.abc", "relationships": {"catalog": {"data": []}}}]});
        assert_eq!(catalog_id_of(&none), None);
        assert_eq!(catalog_id_of(&json!({"data": []})), None);
    }

    #[test]
    fn release_dates_read_like_apples() {
        assert_eq!(fmt_release("2024-10-04"), "OCT 4, 2024");
        assert_eq!(fmt_release("2024-10"), "OCT 2024");
        assert_eq!(fmt_release("1999"), "1999");
        assert_eq!(fmt_release(""), "");
    }

    #[test]
    fn requests_ask_for_the_views_and_fall_back_without_the_extras() {
        let (path, p) = api::page("123", true);
        assert_eq!(path, "/v1/catalog/{{storefrontId}}/artists/123");
        assert!(p["views"].as_str().unwrap().contains("top-songs"));
        assert_eq!(p["extend"], "editorialArtwork,artistBio");
        assert_eq!(p["limit[artists:top-songs]"], 20);
        assert!(api::page("123", false).1.get("limit[artists:top-songs]").is_none());
    }
}
