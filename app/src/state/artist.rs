//! Loading a catalog artist's page (`crate::artist`): a library artist resolves to its catalog
//! artist first; with no match (or when Apple refuses the page twice) the page falls back to the
//! old grid of the artist's albums. Pages are kept for the session, keyed by the route's id, so
//! going back never asks Apple again. Shelves fetch more pages only when scrolled to their end
//! (`artist_more`) or opened in full (`artist_see_all`).

use crate::artist::{self, ArtistPage, Shelf, TOP_SONGS_MAX, ViewPage};
use crate::models::{Artist, Collection, Page};

use super::{Load, Msg, State, api};

/// Which list of an artist page a later page is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum More {
    Top,
    Shelf(Shelf),
}

/// Page size asked for when following a view's `next` (Apple's default is small); a refusal
/// retries once with Apple's default.
const MORE_LIMIT: u32 = 25;

/// See All stops fetching past this many items.
const SEE_ALL_CAP: usize = 300;

impl State {
    /// Starts loading the page of route id `id` unless it is loaded, loading or fell back.
    pub(super) fn ensure_artist(&mut self, id: &str, me: bool) {
        if !matches!(self.artist_pages.get(id), None | Some(Load::Failed(_))) {
            if matches!(self.artist_pages.get(id), Some(Load::Ready(None))) {
                self.ensure_artist_albums(id);
            }
            return;
        }
        let library = Artist::is_library_id(id);
        if library && !me {
            return;
        }
        self.artist_pages.insert(id.to_string(), Load::Loading);
        if library {
            let rid = id.to_string();
            self.request(artist::api::library_artist(id), artist::catalog_id_of, move |result| Msg::ArtistCatalog { id: rid, result });
        } else {
            self.request_artist_page(id, id, true);
        }
    }

    fn request_artist_page(&self, id: &str, catalog: &str, full: bool) {
        let (rid, cid) = (id.to_string(), catalog.to_string());
        self.request(artist::api::page(catalog, full), ArtistPage::parse, move |result| Msg::ArtistPage {
            id: rid,
            catalog: cid,
            full,
            result,
        });
    }

    /// The old page: the artist's albums in one grid (`api::artist_albums`).
    pub(super) fn ensure_artist_albums(&mut self, id: &str) {
        if !matches!(self.artist_albums.get(id), None | Some(Load::Failed(_))) {
            return;
        }
        self.artist_albums.insert(id.to_string(), Load::Loading);
        let reply_id = id.to_string();
        self.request(api::artist_albums(id), |v| Page::parse(v, Collection::parse).items, move |result| Msg::ArtistAlbums {
            id: reply_id,
            result,
        });
    }

    fn artist_fallback(&mut self, id: &str) {
        self.artist_pages.insert(id.to_string(), Load::Ready(None));
        self.ensure_artist_albums(id);
    }

    pub(super) fn on_artist_catalog(&mut self, id: String, result: Result<Option<String>, String>) {
        match result {
            Ok(Some(cid)) => self.request_artist_page(&id, &cid, true),
            // No catalog match (or Apple can't say): the library's albums, as before.
            Ok(None) | Err(_) => self.artist_fallback(&id),
        }
    }

    pub(super) fn on_artist_page(&mut self, id: String, catalog: String, full: bool, result: Result<Option<ArtistPage>, String>) {
        match result {
            Ok(Some(p)) => {
                self.artist_pages.insert(id.clone(), Load::Ready(Some(p)));
                // A See All page restored at launch: it was opened before its page landed.
                if let super::Route::ArtistShelf { id: rid, shelf, .. } = &self.route {
                    if *rid == id {
                        let shelf = *shelf;
                        self.artist_see_all(&id, shelf);
                    }
                }
            }
            Ok(None) => self.artist_fallback(&id),
            Err(e) if full => {
                eprintln!("uzumaki: artist {catalog}: {e}; asking again without the extras");
                self.request_artist_page(&id, &catalog, false);
            }
            Err(e) => {
                eprintln!("uzumaki: artist {catalog}: {e}; showing the albums grid");
                self.artist_fallback(&id);
            }
        }
    }

    fn page_mut(&mut self, id: &str) -> Option<&mut ArtistPage> {
        match self.artist_pages.get_mut(id) {
            Some(Load::Ready(Some(p))) => Some(p),
            _ => None,
        }
    }

    /// Fetches the next page of `what` if it has one and none is on its way. From a shelf's end
    /// coming into view, and from See All. True when a request went out.
    pub fn artist_more(&mut self, id: &str, what: More) -> bool {
        let Some(p) = self.page_mut(id) else { return false };
        let next = match what {
            More::Top => {
                if p.top_loading || p.top_songs.len() >= TOP_SONGS_MAX {
                    return false;
                }
                let Some(n) = p.top_next.clone() else { return false };
                p.top_loading = true;
                n
            }
            More::Shelf(sh) => {
                let sec = p.section_mut(sh);
                if sec.loading {
                    return false;
                }
                let Some(n) = sec.next.clone() else { return false };
                sec.loading = true;
                n
            }
        };
        self.request_more(id, what, &next, true);
        true
    }

    fn request_more(&self, id: &str, what: More, next: &str, limited: bool) {
        let rid = id.to_string();
        let limit = limited.then_some(match what {
            More::Top => 10,
            More::Shelf(_) => MORE_LIMIT,
        });
        self.request(artist::api::more(next, limit), ViewPage::parse, move |result| Msg::ArtistMore { id: rid, what, limited, result });
    }

    /// The See All page of `shelf` opened: fetch its pages until there are none.
    pub fn artist_see_all(&mut self, id: &str, shelf: Shelf) {
        let Some(p) = self.page_mut(id) else { return };
        p.section_mut(shelf).all = true;
        self.artist_more(id, More::Shelf(shelf));
    }

    pub(super) fn on_artist_more(&mut self, id: String, what: More, limited: bool, result: Result<ViewPage, String>) {
        let retry = match (&result, what) {
            (Err(_), _) if limited => self.page_mut(&id).and_then(|p| match what {
                More::Top => p.top_next.clone(),
                More::Shelf(sh) => p.section(sh).next.clone(),
            }),
            _ => None,
        };
        if let Some(next) = retry {
            // Maybe the page size: ask once more with Apple's default.
            self.request_more(&id, what, &next, false);
            return;
        }
        let Some(p) = self.page_mut(&id) else { return };
        let again = match what {
            More::Top => {
                p.top_loading = false;
                if let Ok(vp) = result {
                    for x in vp.songs {
                        if p.top_songs.len() < TOP_SONGS_MAX && !p.top_songs.iter().any(|t| t.id == x.id) {
                            p.top_songs.push(x);
                        }
                    }
                    p.top_next = vp.next.filter(|_| p.top_songs.len() < TOP_SONGS_MAX);
                } else {
                    p.top_next = None;
                }
                false
            }
            More::Shelf(sh) => {
                let sec = p.section_mut(sh);
                sec.loading = false;
                match result {
                    Ok(vp) => {
                        let before = sec.items.len();
                        sec.merge(vp.items, sh.by_date());
                        // A page that adds nothing would loop for ever.
                        sec.next = vp.next.filter(|_| sec.items.len() > before);
                        sec.error = None;
                    }
                    Err(e) => {
                        sec.error = Some(e);
                        sec.next = None;
                    }
                }
                sec.all && sec.next.is_some() && sec.items.len() < SEE_ALL_CAP
            }
        };
        if again {
            self.artist_more(&id, what);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::Route;

    fn online() -> State {
        let mut s = State::offline();
        s.session.bridge_up = true;
        s.session.authorized = true;
        s
    }

    fn sent(s: &State, needle: &str) -> usize {
        s.sent.borrow().iter().filter(|p| p.contains(needle)).count()
    }

    fn fixture() -> ArtistPage {
        crate::scenes::artist_fixture()
    }

    fn open(s: &mut State, id: &str) {
        s.navigate(Route::Artist { id: id.into(), name: "Mara Quell".into() });
    }

    #[test]
    fn a_library_artist_opens_its_catalog_page_and_back_never_asks_again() {
        let mut s = online();
        open(&mut s, "r.12");
        assert_eq!(sent(&s, "/v1/me/library/artists/r.12 "), 1);
        assert!(s.sent.borrow().last().unwrap().contains("\"include\":\"catalog\""));
        s.apply_msg(Msg::ArtistCatalog { id: "r.12".into(), result: Ok(Some("28673423".into())) });
        assert_eq!(sent(&s, "/v1/catalog/{{storefrontId}}/artists/28673423 "), 1);
        assert!(s.sent.borrow().last().unwrap().contains("views"));
        s.apply_msg(Msg::ArtistPage { id: "r.12".into(), catalog: "28673423".into(), full: true, result: Ok(Some(fixture())) });
        assert!(matches!(s.artist_pages.get("r.12"), Some(Load::Ready(Some(p))) if p.id == "28673423"));
        assert_eq!(sent(&s, "/artists/r.12/albums"), 0, "no albums grid");
        let before = s.sent.borrow().len();
        s.navigate(Route::Home);
        s.back();
        assert_eq!(s.route, Route::Artist { id: "r.12".into(), name: "Mara Quell".into() });
        assert!(s.sent.borrow()[before..].iter().all(|p| !p.contains("artists")));
    }

    #[test]
    fn a_library_artist_without_a_catalog_match_keeps_the_albums_grid() {
        let mut s = online();
        open(&mut s, "r.7");
        s.apply_msg(Msg::ArtistCatalog { id: "r.7".into(), result: Ok(None) });
        assert_eq!(s.artist_pages.get("r.7"), Some(&Load::Ready(None)));
        assert_eq!(sent(&s, "/v1/me/library/artists/r.7/albums"), 1);
        assert_eq!(sent(&s, "/v1/catalog/"), 0);
    }

    #[test]
    fn a_refused_page_asks_once_without_the_extras_then_falls_back_to_the_grid() {
        let mut s = online();
        open(&mut s, "555");
        assert_eq!(sent(&s, "limit[artists:top-songs]"), 1);
        s.apply_msg(Msg::ArtistPage { id: "555".into(), catalog: "555".into(), full: true, result: Err("HTTP 400".into()) });
        assert_eq!(sent(&s, "/artists/555 "), 2);
        assert!(!s.sent.borrow().last().unwrap().contains("limit[artists:top-songs]"));
        s.apply_msg(Msg::ArtistPage { id: "555".into(), catalog: "555".into(), full: false, result: Err("HTTP 400".into()) });
        assert_eq!(s.artist_pages.get("555"), Some(&Load::Ready(None)));
        assert_eq!(sent(&s, "/v1/catalog/{{storefrontId}}/artists/555/albums"), 1, "the old grid");
    }

    #[test]
    fn shelves_page_lazily_and_see_all_fetches_until_the_end() {
        let mut s = online();
        s.artist_pages.insert("28673423".into(), Load::Ready(Some(fixture())));
        open(&mut s, "28673423");
        assert_eq!(s.sent.borrow().iter().filter(|p| p.contains("artists")).count(), 0, "cached: nothing asked");
        // The Albums shelf's end comes near the window.
        assert!(s.artist_more("28673423", More::Shelf(Shelf::Albums)));
        assert!(!s.artist_more("28673423", More::Shelf(Shelf::Albums)), "one request at a time");
        assert_eq!(sent(&s, "view/full-albums?offset=9 {\"limit\":25}"), 1);
        let older = serde_json::json!({"data": [{"id": "x1", "type": "albums", "attributes": {"name": "Old", "releaseDate": "1990-01-01"}}],
            "next": "/v1/catalog/us/artists/28673423/view/full-albums?offset=10"});
        s.apply_msg(Msg::ArtistMore { id: "28673423".into(), what: More::Shelf(Shelf::Albums), limited: true, result: Ok(ViewPage::parse(&older)) });
        let p = match s.artist_pages.get("28673423") { Some(Load::Ready(Some(p))) => p.clone(), _ => panic!() };
        assert_eq!(p.section(Shelf::Albums).items.last().unwrap().id, "x1", "sorted in, oldest last");
        assert!(p.section(Shelf::Albums).next.is_some());
        // See All keeps going by itself until Apple has no more.
        s.navigate(Route::ArtistShelf { id: "28673423".into(), name: "Mara Quell".into(), shelf: Shelf::Albums });
        assert_eq!(sent(&s, "view/full-albums?offset=10"), 1);
        let newer = serde_json::json!({"data": [{"id": "x2", "type": "albums", "attributes": {"name": "New", "releaseDate": "2030-01-01"}}]});
        s.apply_msg(Msg::ArtistMore { id: "28673423".into(), what: More::Shelf(Shelf::Albums), limited: true, result: Ok(ViewPage::parse(&newer)) });
        let p = match s.artist_pages.get("28673423") { Some(Load::Ready(Some(p))) => p.clone(), _ => panic!() };
        assert_eq!(p.section(Shelf::Albums).items[0].id, "x2", "newest first");
        assert!(p.section(Shelf::Albums).next.is_none());
        assert!(!s.artist_more("28673423", More::Shelf(Shelf::Albums)));
    }

    #[test]
    fn a_refused_page_size_retries_with_apples_default() {
        let mut s = online();
        s.artist_pages.insert("28673423".into(), Load::Ready(Some(fixture())));
        assert!(s.artist_more("28673423", More::Top));
        s.apply_msg(Msg::ArtistMore { id: "28673423".into(), what: More::Top, limited: true, result: Err("HTTP 400".into()) });
        assert_eq!(s.sent.borrow().last().unwrap(), "/v1/catalog/us/artists/28673423/view/top-songs?offset=14 {}");
    }
}
