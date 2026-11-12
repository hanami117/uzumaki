//! The New / Edit Playlist dialog: a centred modal card in the Settings style.
//!
//! New (Ctrl+N, the sidebar's +, "Add to Playlist → New Playlist…"): Create calls
//! `State::create_playlist`; the dialog stays open while Apple answers and closes once the app
//! has moved to the new playlist. Edit ("Edit Playlist…" / "Change Picture…" in the playlist
//! menus, the page's cover): name and description, only for playlists Apple lets the user
//! rename, and the picture that only Uzumaki shows. Save applies both and closes.

use std::path::PathBuf;

use rune::{Cursor, El, Key, KeyEvent, TextEdit, TextStyle, Tx, col, el, row, text, text_input};

use super::widgets::{art, small_button, Size};
use crate::models::{Artwork, Collection, Song};
use crate::state::{Route, State};
use crate::theme::*;

#[derive(Default)]
enum Mode {
    #[default]
    New,
    /// The playlist being edited, as it was when the dialog opened.
    Edit(Box<Collection>),
}

/// What Save / Create does with the picture.
#[derive(Default)]
enum Picture {
    #[default]
    Keep,
    Set(PathBuf),
    /// Back to Apple's artwork.
    Remove,
}

#[derive(Default)]
pub struct Dialog {
    open: bool,
    mode: Mode,
    songs: Vec<Song>,
    name: TextEdit,
    desc: TextEdit,
    picture: Picture,
    /// The page the app was on when Create was pressed: a different one means it worked.
    route: Option<Route>,
    /// `creating_playlist` at the last poll.
    was_creating: bool,
}

/// Opens the dialog; `songs` go into the new playlist (empty for an empty playlist).
pub fn open(s: &mut State, songs: Vec<Song>) {
    // A fresh `Dialog` forgets the last open's mode, texts and picture.
    s.ui.new_pl = Dialog { open: true, songs, ..Dialog::default() };
    s.ui.new_pl.name.focused = true;
}

/// Opens the dialog on an existing playlist.
pub fn open_edit(s: &mut State, c: &Collection) {
    let renamable = State::can_rename(c);
    let mut d = Dialog { open: true, mode: Mode::Edit(Box::new(c.clone())), ..Dialog::default() };
    if renamable {
        d.name = TextEdit::new(c.title.clone());
        d.desc = TextEdit::new(c.description.clone().unwrap_or_default());
        d.name.focused = true;
    }
    s.ui.new_pl = d;
}

/// Whether the dialog is showing (it is modal, like Settings).
pub fn is_open(s: &State) -> bool {
    s.ui.new_pl.open
}

pub fn close(s: &mut State) {
    s.ui.new_pl.open = false;
    s.ui.new_pl.songs.clear();
    s.ui.new_pl.picture = Picture::Keep;
}

/// Closes the dialog when a create that was running ended with the app on another page.
/// True when it closed.
pub fn poll(s: &mut State) -> bool {
    let now = s.creating_playlist;
    let d = &mut s.ui.new_pl;
    let was = std::mem::replace(&mut d.was_creating, now);
    if d.open && was && !now && d.route.as_ref().is_some_and(|r| *r != s.route) {
        close(s);
        return true;
    }
    false
}

/// Name and Description show in New, and in Edit for playlists that can be renamed.
fn has_fields(d: &Dialog) -> bool {
    match &d.mode {
        Mode::New => true,
        Mode::Edit(c) => State::can_rename(c),
    }
}

fn can_submit(s: &State) -> bool {
    let d = &s.ui.new_pl;
    let named = !has_fields(d) || !d.name.text.trim().is_empty();
    named && !s.creating_playlist
}

fn submit(s: &mut State) {
    if !can_submit(s) {
        return;
    }
    let (name, desc) = {
        let d = &s.ui.new_pl;
        (d.name.text.trim().to_string(), d.desc.text.trim().to_string())
    };
    let picture = std::mem::take(&mut s.ui.new_pl.picture);
    match &s.ui.new_pl.mode {
        Mode::New => {
            let (route, songs) = (s.route.clone(), s.ui.new_pl.songs.clone());
            s.ui.new_pl.route = Some(route);
            s.set_new_playlist_picture(match picture {
                Picture::Set(p) => Some(p),
                _ => None,
            });
            s.create_playlist(&name, &desc, &songs);
            s.ui.new_pl.was_creating = s.creating_playlist;
        }
        Mode::Edit(c) => {
            let (id, renamable) = (c.id.clone(), has_fields(&s.ui.new_pl));
            if renamable {
                s.edit_playlist(&id, &name, &desc);
            }
            match picture {
                Picture::Set(p) => s.set_playlist_picture(&id, p),
                Picture::Remove => s.remove_playlist_picture(&id),
                Picture::Keep => {}
            }
            close(s);
        }
    }
}

/// Whether the picture shown has anything for Remove to take away.
fn has_picture(s: &State) -> bool {
    let d = &s.ui.new_pl;
    match (&d.picture, &d.mode) {
        (Picture::Set(_), _) => true,
        (Picture::Remove, _) => false,
        (Picture::Keep, Mode::New) => false,
        (Picture::Keep, Mode::Edit(c)) => s.custom_art(&c.id).is_some(),
    }
}

fn remove_picture(s: &mut State) {
    let had_saved = matches!(&s.ui.new_pl.mode, Mode::Edit(c) if s.custom_art(&c.id).is_some());
    s.ui.new_pl.picture = if had_saved { Picture::Remove } else { Picture::Keep };
}

fn choose_picture(tx: &mut Tx) {
    tx.open_file(
        rune::FileDialogOptions { title: "Choose a playlist picture".into(), ..Default::default() }
            .filter("Pictures", &["png", "jpg", "jpeg"]),
        |s: &mut State, result: rune::FileDialogResult, _tx: &mut Tx| match result {
            Ok(Some(path)) => s.ui.new_pl.picture = Picture::Set(path),
            Ok(None) => {} // cancelled
            Err(e) => s.toast = Some(format!("Couldn't open the file picker: {e}")),
        },
    );
}

/// The 88 pt preview of the picture the playlist will have, and the buttons that change it.
fn picture_part(s: &State) -> (El, El) {
    let d = &s.ui.new_pl;
    let shown: Option<Artwork> = match (&d.picture, &d.mode) {
        (Picture::Set(p), _) => Some(Artwork { template: format!("file://{}", p.display()), bg: None }),
        (Picture::Remove, Mode::Edit(c)) => c.artwork.clone(),
        (Picture::Keep, Mode::Edit(c)) => s.cover_of(c).cloned(),
        _ => None,
    };
    let title = match &d.mode {
        Mode::Edit(c) if !has_fields(d) => c.title.as_str(),
        _ => d.name.text.trim(),
    };
    let preview = art(s, shown.as_ref(), title, Size::Fixed(88.0), 8.0, 26.0).key("new-playlist/preview");

    let mut buttons = row().gap(8.0).child(small_button("new-playlist/choose", "Choose Picture\u{2026}").on_press_tx(|_s: &mut State, tx: &mut Tx| choose_picture(tx)));
    if has_picture(s) {
        buttons = buttons.child(small_button("new-playlist/remove-picture", "Remove").on_press(remove_picture));
    }
    let controls = col().w_full().gap(8.0).child(buttons)
        .child(text("The picture shows only in Uzumaki.").font(SMALL12).color(PAL.dim));
    (preview, controls)
}

fn field(
    key: &'static str,
    label: &'static str,
    edit: &TextEdit,
    placeholder: &'static str,
    get: fn(&mut State) -> &mut TextEdit,
) -> El {
    let input = text_input(edit, TextStyle::new(14.0, 400.0))
        .key(key).grow(1.0).single_line().color(PAL.text).placeholder(placeholder, PAL.dim)
        .on_edit(get)
        .on_submit(|s: &mut State, _tx: &mut Tx| submit(s))
        // The field would swallow Esc (it just drops focus), so close from here.
        .on_input_key(|s: &mut State, ev: &KeyEvent, tx: &mut Tx| {
            if super::keys::track_focus(s, ev, tx) {
                return true; // the marker after a Tab (`keys::track_focus`)
            }
            if ev.key == Key::Escape {
                close(s);
                tx.blur();
                return true;
            }
            false
        });
    col().w_full().gap(6.0)
        .child(text(label).font(SMALL12).color(PAL.dim))
        .child(
            row().key(format!("{key}-box")).role("textbox").w_full().h(38.0).px(12.0).center_y().radius(8.0)
                .fill(PAL.field).border(1.0, PAL.field_border).cursor(Cursor::Text).child(input),
        )
}

/// The dialog on its dimmed backdrop, or `None` while closed. `win` is the window size.
pub fn overlay(s: &State, win: [f32; 2]) -> Option<El> {
    let d = &s.ui.new_pl;
    if !d.open {
        return None;
    }
    let w = 420.0_f32.min(win[0] - 48.0).max(280.0);

    let editing = matches!(d.mode, Mode::Edit(_));
    let (preview, controls) = picture_part(s);
    let top = if has_fields(d) {
        // Like Apple's edit sheet: the picture left, the text fields right.
        let fields = col().grow(1.0).min_w(0.0).gap(16.0)
            .child(field("new-playlist/name", "Name", &d.name, "Playlist name", |s| &mut s.ui.new_pl.name))
            .child(field("new-playlist/desc", "Description (optional)", &d.desc, "Add a description", |s| &mut s.ui.new_pl.desc));
        col().w_full().gap(16.0)
            .child(row().w_full().gap(16.0).child(preview).child(fields))
            .child(controls)
    } else {
        row().w_full().gap(16.0).child(preview).child(el().grow(1.0).min_w(0.0).child(controls))
    };

    let mut card = col().key("new-playlist/card").w(w).padding(24.0).gap(16.0).radius(14.0).fill(PAL.player)
        .border(1.0, PAL.field_border).shadow(12.0, 40.0, crate::theme::shadow(0.5))
        // Presses on the card stop here instead of reaching the backdrop (which closes).
        .on_press(|_: &mut State| {})
        .child(text(if editing { "Edit Playlist" } else { "New Playlist" }).font(H2).color(PAL.text))
        .child(top);

    if !d.songs.is_empty() {
        let n = d.songs.len();
        let mut info = col().key("new-playlist/songs").w_full().gap(2.0).child(
            text(format!("With {n} song{}", if n == 1 { "" } else { "s" })).font(SMALL13).color(PAL.text2),
        );
        for song in d.songs.iter().take(3) {
            info = info.child(text(format!("{} \u{b7} {}", song.title, song.artist)).font(SMALL13).color(PAL.dim).truncate());
        }
        if n > 3 {
            info = info.child(text(format!("and {} more", n - 3)).font(SMALL13).color(PAL.dim));
        }
        card = card.child(info);
    }

    let ok = can_submit(s);
    let label = match (editing, s.creating_playlist) {
        (true, _) => "Save",
        (false, true) => "Creating\u{2026}",
        (false, false) => "Create",
    };
    let create = el().key("new-playlist/create").role("button").label(label).h(32.0).px(18.0).radius(16.0).center()
        .fill(PAL.accent)
        .child(text(label).font(TextStyle::new(13.0, 600.0)).color(PAL.ink));
    let create = if ok {
        create.focusable(true).hover(crate::theme::ov(0.12)).press(rune::Color::BLACK.alpha(0.12)).cursor(Cursor::Pointer)
            .on_press(submit)
    } else {
        create.opacity(0.4).cursor(Cursor::NotAllowed)
    };
    card = card.child(
        row().w_full().gap(8.0).justify_end().mt(4.0)
            .child(small_button("new-playlist/cancel", "Cancel").on_press(close))
            .child(create),
    );

    Some(
        el().key("new-playlist").pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0)).center()
            .fill(crate::theme::scrim(0.55)).on_press(close).child(card),
    )
}
