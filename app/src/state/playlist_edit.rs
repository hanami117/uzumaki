//! Editing library playlists: the name and description on Apple's side, and a picture that
//! only Uzumaki shows.
//!
//! Renaming uses music.apple.com's own call (from its bundle, 2026-11-09):
//! `PATCH /v1/me/library/playlists/{id}` with `{"attributes": {"name", "description",
//! "isPublic"}}`, adding `?with=shared` for a public playlist.
//!
//! Apple's web API can't change a playlist's artwork. A chosen picture is turned upright, cropped
//! to its centred square, scaled down and saved under `settings::playlist_art_dir()`.
//! `Settings::playlist_art` points at that file, and every playlist cover goes through
//! `State::cover_of`.

use std::path::{Path, PathBuf};

use image::{DynamicImage, ImageDecoder, ImageReader};
use serde_json::json;

use super::*;
use crate::models::Artwork;

/// Saved pictures are at most this many pixels a side: the page header draws covers at 220 pt.
const PICTURE_PX: u32 = 800;

/// Crops `src` to its centred square, scales it to at most `PICTURE_PX` and writes it as a
/// JPEG in `dir`. The name carries `stamp`, so the artwork cache never serves an older picture.
pub(crate) fn save_picture(src: &Path, dir: &Path, id: &str, stamp: u128) -> Result<PathBuf, String> {
    let unreadable = |e: &dyn std::fmt::Display| format!("Uzumaki can't read it ({e})");
    let mut decoder = ImageReader::open(src)
        .map_err(|e| e.to_string())?
        .with_guessed_format()
        .map_err(|e| e.to_string())?
        .into_decoder()
        .map_err(|e| unreadable(&e))?;
    let orientation = decoder.orientation().unwrap_or(image::metadata::Orientation::NoTransforms);
    let mut img = DynamicImage::from_decoder(decoder).map_err(|e| unreadable(&e))?;
    img.apply_orientation(orientation); // phone photos are often stored sideways
    let (w, h) = (img.width(), img.height());
    let side = w.min(h);
    if side == 0 {
        return Err("the picture is empty".into());
    }
    let mut square = img.crop_imm((w - side) / 2, (h - side) / 2, side, side);
    if side > PICTURE_PX {
        square = square.resize_exact(PICTURE_PX, PICTURE_PX, image::imageops::FilterType::Lanczos3);
    }
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let safe: String = id.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')).collect();
    let path = dir.join(format!("{safe}-{stamp}.jpg"));
    let file = std::fs::File::create(&path).map_err(|e| e.to_string())?;
    let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(std::io::BufWriter::new(file), 90);
    DynamicImage::ImageRgb8(square.to_rgb8()).write_with_encoder(encoder).map_err(|e| e.to_string())?;
    Ok(path)
}

fn now_millis() -> u128 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis())
}

impl State {
    /// The picture the user chose for playlist `id`, if any.
    pub fn custom_art(&self, id: &str) -> Option<&Artwork> {
        self.settings.playlist_art.get(id)
    }

    /// The cover to draw for an album or playlist: the user's picture, else Apple's.
    pub fn cover_of<'a>(&'a self, c: &'a Collection) -> Option<&'a Artwork> {
        self.custom_art(&c.id).or(c.artwork.as_ref())
    }

    /// The picture of a pinned item. A pinned library playlist whose picture was uploaded keeps a
    /// signed link that dies after a day, so the list's current one wins over the saved one.
    pub fn pin_cover<'a>(&'a self, pin: &'a crate::settings::Pin) -> Option<&'a Artwork> {
        let listed = || self.library_playlists.items.iter().find(|p| p.id == pin.id)?.artwork.as_ref();
        self.custom_art(&pin.id).or_else(listed).or(pin.artwork.as_ref())
    }

    /// Library playlists can get a picture (Favourite Songs too: it stays on this computer).
    pub fn can_set_picture(c: &Collection) -> bool {
        c.kind == CollectionKind::Playlist && c.id.starts_with("p.")
    }

    /// Only the user's own playlists can be renamed on Apple's side.
    pub fn can_rename(c: &Collection) -> bool {
        c.kind == CollectionKind::Playlist && c.id.starts_with("p.") && c.editable
    }

    /// Where pictures are saved: the user's data folder in the app, a scratch folder in tests.
    fn picture_dir(&self) -> PathBuf {
        if self.persist_settings {
            crate::settings::playlist_art_dir()
        } else {
            // Tests run on parallel threads: one folder each.
            let thread = format!("{:?}", std::thread::current().id()).replace(|c: char| !c.is_ascii_alphanumeric(), "");
            std::env::temp_dir().join(format!("uzumaki-test-art-{}-{thread}", std::process::id()))
        }
    }

    /// Makes `src` the picture of playlist `id`. The resizing runs on a worker thread
    /// (synchronously in headless runs and tests); `picture_saved` lands it.
    pub fn set_playlist_picture(&mut self, id: &str, src: PathBuf) {
        let (dir, id, stamp) = (self.picture_dir(), id.to_string(), now_millis());
        match &self.handle {
            Some(handle) => {
                let handle = handle.clone();
                std::thread::spawn(move || {
                    let result = save_picture(&src, &dir, &id, stamp);
                    handle.update(move |s: &mut State, tx| {
                        s.landing(tx);
                        s.picture_saved(id, result);
                    });
                });
            }
            None => {
                let result = save_picture(&src, &dir, &id, stamp);
                self.picture_saved(id, result);
            }
        }
    }

    pub(super) fn picture_saved(&mut self, id: String, result: Result<PathBuf, String>) {
        match result {
            Ok(path) => {
                let art = Artwork { template: format!("file://{}", path.display()), bg: None };
                if let Some(old) = self.settings.playlist_art.insert(id, art) {
                    self.delete_picture_file(&old);
                }
                self.save_settings();
            }
            Err(e) => self.toast = Some(format!("Couldn't use that picture: {e}")),
        }
    }

    /// Back to Apple's artwork for playlist `id`.
    pub fn remove_playlist_picture(&mut self, id: &str) {
        if let Some(old) = self.settings.playlist_art.remove(id) {
            self.delete_picture_file(&old);
            self.save_settings();
        }
    }

    /// Deletes a saved picture, only ever one in our own folder.
    fn delete_picture_file(&self, art: &Artwork) {
        if let Some(path) = art.template.strip_prefix("file://").map(Path::new) {
            if path.starts_with(self.picture_dir()) {
                let _ = std::fs::remove_file(path);
            }
        }
    }

    /// The New Playlist dialog's picture, applied once Apple has created the playlist.
    pub fn set_new_playlist_picture(&mut self, src: Option<PathBuf>) {
        self.new_playlist_picture = src;
    }

    /// Renames playlist `id` and sets its description on Apple's side. The app shows the
    /// change at once and turns it back if Apple refuses.
    pub fn edit_playlist(&mut self, id: &str, name: &str, description: &str) {
        let found = self.library_playlists.items.iter().find(|c| c.id == id).or_else(|| self.collections.get(id).and_then(|l| l.ready()));
        let Some(c) = found.cloned() else { return };
        let (name, desc) = (name.trim(), description.trim());
        if name.is_empty() || (name == c.title && desc == c.description.as_deref().unwrap_or("")) {
            return;
        }
        if !Self::can_rename(&c) {
            self.toast = Some("Only your own playlists can be renamed.".into());
            return;
        }
        if !self.session.authorized {
            self.toast = Some("Sign in to Apple Music to rename playlists.".into());
            return;
        }
        let mut attributes = json!({ "name": name, "description": desc });
        if let Some(public) = c.public {
            attributes["isPublic"] = json!(public);
        }
        let mut path = format!("/v1/me/library/playlists/{id}");
        if c.public == Some(true) {
            path.push_str("?with=shared");
        }
        let old = (c.title.clone(), c.description.clone());
        self.show_playlist_text(id, name.to_string(), (!desc.is_empty()).then(|| desc.to_string()));
        let (id, new) = (id.to_string(), name.to_string());
        self.write("PATCH", path, Some(json!({ "attributes": attributes })), move |result| Msg::PlaylistEdited { id, old, new, result });
    }

    /// Puts a playlist's name and description everywhere the app keeps a copy.
    fn show_playlist_text(&mut self, id: &str, title: String, description: Option<String>) {
        let set = |c: &mut Collection| {
            c.title = title.clone();
            c.description = description.clone();
        };
        self.library_playlists.items.iter_mut().filter(|c| c.id == id).for_each(set);
        self.save(cache::PLAYLISTS, self.library_playlists.items.clone());
        self.recently_added.items.iter_mut().filter(|c| c.id == id).for_each(set);
        if let Some(Load::Ready(c)) = self.collections.get_mut(id) {
            set(c);
            let c = c.clone();
            self.save_collection(&c);
        }
        let mut pinned = false;
        for p in self.settings.pins.iter_mut().filter(|p| p.id == id) {
            p.title = title.clone();
            pinned = true;
        }
        if pinned {
            self.save_settings();
        }
    }

    /// Apple's answer to a rename: on a refusal, the old name and description come back.
    pub(super) fn playlist_edited(&mut self, id: String, old: (String, Option<String>), new: String, result: Result<(), String>) {
        match result {
            Ok(()) => self.toast = Some(format!("Saved “{new}”")),
            Err(e) => {
                self.show_playlist_text(&id, old.0, old.1);
                self.toast = Some(format!("Couldn't rename the playlist to “{new}”: {e}"));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn playlist(id: &str, editable: bool) -> Collection {
        Collection {
            kind: CollectionKind::Playlist,
            id: id.into(),
            title: "Mix".into(),
            subtitle: String::new(),
            artwork: Some(Artwork { template: "https://apple/{w}x{h}.{f}".into(), bg: None }),
            year: None,
            genre: None,
            description: Some("Old notes".into()),
            track_count: None,
            modified: None,
            editable,
            favorites: false,
            public: Some(false),
            tracks: Vec::new(),
            artists: Vec::new(),
        }
    }

    fn signed_in_with(c: Collection) -> State {
        let mut s = State::offline();
        s.session.authorized = true;
        s.collections.insert(c.id.clone(), Load::Ready(c.clone()));
        s.library_playlists = Paged::ready(vec![c]);
        s
    }

    #[test]
    fn renaming_patches_apple_and_shows_at_once() {
        let mut s = signed_in_with(playlist("p.mix", true));
        s.settings.pins.push(crate::settings::Pin { kind: crate::settings::PinKind::Playlist, id: "p.mix".into(), title: "Mix".into(), artwork: None });
        s.edit_playlist("p.mix", "  Night Mix ", "");
        let sent = s.sent.borrow().last().unwrap().clone();
        assert!(sent.starts_with("PATCH /v1/me/library/playlists/p.mix "), "{sent}");
        assert!(sent.contains(r#""name":"Night Mix""#) && sent.contains(r#""description":"""#) && sent.contains(r#""isPublic":false"#), "{sent}");
        assert_eq!(s.library_playlists.items[0].title, "Night Mix");
        assert_eq!(s.library_playlists.items[0].description, None);
        assert_eq!(s.collections["p.mix"].ready().unwrap().title, "Night Mix");
        assert_eq!(s.settings.pins[0].title, "Night Mix");
        // Apple refuses: everything goes back.
        s.apply_msg(Msg::PlaylistEdited { id: "p.mix".into(), old: ("Mix".into(), Some("Old notes".into())), new: "Night Mix".into(), result: Err("HTTP 403".into()) });
        assert_eq!(s.library_playlists.items[0].title, "Mix");
        assert_eq!(s.collections["p.mix"].ready().unwrap().description.as_deref(), Some("Old notes"));
        assert_eq!(s.settings.pins[0].title, "Mix");
        assert!(s.toast.as_deref().unwrap().contains("HTTP 403"));
    }

    #[test]
    fn a_public_playlist_stays_public_and_unchanged_text_sends_nothing() {
        let mut s = signed_in_with(Collection { public: Some(true), ..playlist("p.pub", true) });
        s.edit_playlist("p.pub", "Mix", "Old notes");
        assert!(s.sent.borrow().is_empty(), "nothing changed");
        s.edit_playlist("p.pub", "Mix 2", "Old notes");
        let sent = s.sent.borrow().last().unwrap().clone();
        assert!(sent.starts_with("PATCH /v1/me/library/playlists/p.pub?with=shared ") && sent.contains(r#""isPublic":true"#), "{sent}");
    }

    #[test]
    fn only_the_users_own_playlists_are_renamed() {
        let mut s = signed_in_with(playlist("p.fav", false));
        s.edit_playlist("p.fav", "Renamed", "");
        assert!(s.sent.borrow().is_empty());
        assert_eq!(s.library_playlists.items[0].title, "Mix");
        assert!(State::can_set_picture(&playlist("p.fav", false)), "a picture is fine: it stays local");
        assert!(!State::can_set_picture(&Collection { kind: CollectionKind::Album, ..playlist("l.alb", false) }));
    }

    #[test]
    fn a_picture_is_cropped_square_saved_and_replaces_apples_cover() {
        let dir = std::env::temp_dir().join(format!("uzumaki-pic-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("wide.png");
        image::RgbImage::from_pixel(1200, 900, image::Rgb([200, 40, 60])).save(&src).unwrap();
        let mut s = signed_in_with(playlist("p.mix", true));
        s.set_playlist_picture("p.mix", src.clone());
        let art = s.custom_art("p.mix").cloned().expect("saved");
        let saved = PathBuf::from(art.template.strip_prefix("file://").unwrap());
        let img = image::open(&saved).unwrap();
        assert_eq!((img.width(), img.height()), (800, 800), "square, scaled down");
        let c = s.collections["p.mix"].ready().unwrap().clone();
        assert_eq!(s.cover_of(&c), Some(&art));
        // A new picture replaces the file; removing it brings Apple's cover back.
        s.set_playlist_picture("p.mix", src.clone());
        assert!(!saved.exists(), "the old copy is deleted");
        let newer = PathBuf::from(s.custom_art("p.mix").unwrap().template.strip_prefix("file://").unwrap().to_string());
        s.remove_playlist_picture("p.mix");
        assert!(!newer.exists());
        assert_eq!(s.cover_of(&c), c.artwork.as_ref());
        assert!(src.exists(), "the user's own file is never touched");
        // Not a picture.
        let junk = dir.join("junk.png");
        std::fs::write(&junk, b"not an image").unwrap();
        s.set_playlist_picture("p.mix", junk);
        assert!(s.custom_art("p.mix").is_none());
        assert!(s.toast.as_deref().unwrap().starts_with("Couldn't use that picture"));
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(s.picture_dir());
    }

    #[test]
    fn a_new_playlists_picture_lands_once_apple_has_made_it() {
        let dir = std::env::temp_dir().join(format!("uzumaki-pic-new-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("sq.png");
        image::RgbImage::from_pixel(64, 64, image::Rgb([10, 200, 60])).save(&src).unwrap();
        let mut s = State::offline();
        s.session.authorized = true;
        s.set_new_playlist_picture(Some(src));
        s.apply_msg(Msg::PlaylistCreated { name: "Fresh".into(), songs: vec![], result: Ok(playlist("p.new", true)) });
        assert!(s.custom_art("p.new").is_some());
        assert!(s.new_playlist_picture.is_none());
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(s.picture_dir());
    }
}
