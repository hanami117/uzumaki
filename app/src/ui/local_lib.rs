//! Local only mode's library tabs over the music folders: Albums (the card grid), a local
//! artist's page (their albums, then their songs) and Folders (the disk as it is). Songs is the
//! Local Files page; an album opens on the album page (`collection.rs`).
//!
//! Everything comes from `LocalLibrary::view`, derived once per scan on the scan thread; the
//! songs of the artist or folder on screen are `LocalLibrary::page`, filled on navigation. The
//! view only looks things up.

use std::path::{Path, PathBuf};

use rune::{Cursor, El, GrowthSpec, ScrollSpec, Tx, col, el, icon, row, text};

use super::grids::{self, GridSrc};
use super::songs_table::{self as table, Which};
use super::widgets::{Grid, card, heading, muted, pill, status};
use super::{PAGE_PX, ctx_menu, filter, icons, lists, local_files};
use crate::state::{LocalWhat, Route, State};
use crate::theme::*;

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{} {}", local_files::thousands(n), if n == 1 { one } else { many })
}

fn head(title: impl Into<String>, sub: String) -> El {
    col().w_full().px(PAGE_PX).pt(28.0).pb(16.0).gap(4.0).child(heading(title).min_w(0.0).truncate()).child(muted(sub))
}

/// Play and Shuffle over the page's songs as listed (sorted, filtered).
fn play_buttons(more: El) -> El {
    row().gap(10.0).mt(14.0).center_y()
        .child(pill("group-play", "Play", &icons::PLAY, PAL.accent, PAL.ink).on_press(|s: &mut State| play_visible(s, false)))
        .child(pill("group-shuffle", "Shuffle", &icons::SHUFFLE, PAL.nav_active, PAL.text).on_press(|s: &mut State| play_visible(s, true)))
        .child(more)
}

fn play_visible(s: &mut State, shuffle: bool) {
    let songs = lists::visible(s);
    let name = lists::name(s);
    if shuffle {
        s.shuffle_play(&songs, &name);
    } else {
        s.play_list(&songs, 0, &name);
    }
}

fn label(text_: &str) -> El {
    row().w_full().px(PAGE_PX).pt(18.0).pb(8.0).child(text(text_.to_string()).font(H2).color(PAL.text))
}

// ---------------------------------------------------------------- Albums

pub fn albums_page(s: &State, g: Grid) -> El {
    let all = s.local_lib.view.albums.len();
    let sub = if all > 0 { plural(all, "album", "albums") + " in your music folders" } else { String::new() };
    let page = col().key("local-albums").grow(1.0).min_h(0.0).w_full().child(head("Albums", sub));
    if let Some(p) = local_files::placeholder(s) {
        return page.child(p);
    }
    let shown = lists::album_matches(s);
    let n = shown.as_ref().map_or(all, |m| m.len());
    let tag = format!("{}{}", s.local_lib.generation, lists::query(s));
    page.child(filter::bar(s, n, all).unwrap_or_else(col))
        .child(grids::grid(s, "local-albums-grid".into(), GridSrc::LocalAlbums, g, false, shown, tag))
}

// ---------------------------------------------------------------- an artist

/// A local artist: header, their albums as cards, then their songs as the songs table, all one
/// virtual list (like the album page).
pub fn artist_page(s: &State, id: &str, name: &str, g: Grid, avail: f32) -> El {
    let key = format!("local-artist-{id}");
    let v = &s.local_lib.view;
    let Some(a) = v.artist(id) else {
        let msg = if s.local_lib.loaded { "This artist is no longer in your music folders." } else { "Loading\u{2026}" };
        return col().key(key).grow(1.0).min_h(0.0).w_full().child(head(name.to_string(), String::new()))
            .child(col().w_full().px(PAGE_PX).child(status(msg, false)));
    };
    let n_albums = v.artist_albums[a].len();
    let album_rows = n_albums.div_ceil(g.n);
    let n_songs = v.artist_songs[a].len();
    let order = lists::order(s);
    let m = order.len();
    let cols = s.ui.table.cols(avail);
    let sort = s.ui.table.sort(Which::Group);
    let query = lists::query(s);
    let list = lists::list_id(s).unwrap_or_default();
    let cur_id = s.playback.song.as_ref().map(|x| x.id.clone());
    // Rows: header, ["Albums", album rows…], "Songs", table head, songs.
    let albums_at = 1;
    let songs_label = if n_albums > 0 { albums_at + 1 + album_rows } else { 1 };
    let count = songs_label + 2 + m.max(1);
    let (aid, aname) = (id.to_string(), name.to_string());
    let rows = col().key("rows").growth(GrowthSpec::default())
        .virtual_rows(
            count,
            50.0,
            |_: &State, i| format!("r{i}").into(),
            move |s: &State, i| {
                let v = &s.local_lib.view;
                let Some(a) = v.artist(&aid) else { return col() };
                if i == 0 {
                    let mut sub = Vec::new();
                    if n_albums > 0 {
                        sub.push(plural(n_albums, "album", "albums"));
                    }
                    sub.push(plural(n_songs, "song", "songs"));
                    let artist = crate::models::Artist { id: aid.clone(), name: aname.clone() };
                    let more = ctx_menu::more_button("artist-more", 44.0, move |s| ctx_menu::artist_items(s, &artist));
                    return head(aname.clone(), sub.join(" \u{b7} ")).child(play_buttons(more)).pb(8.0);
                }
                if n_albums > 0 && i == albums_at {
                    return label("Albums");
                }
                if n_albums > 0 && i > albums_at && i < songs_label {
                    let r0 = i - albums_at - 1;
                    let mut r = row().w_full().gap(12.0).py(6.0).items_start();
                    for &k in v.artist_albums[a].iter().skip(r0 * g.n).take(g.n) {
                        if let Some(c) = v.albums.get(k as usize) {
                            r = r.child(card(s, c, g.w));
                        }
                    }
                    return row().w_full().px(PAGE_PX - 8.0).child(r);
                }
                if i == songs_label {
                    return label("Songs");
                }
                if i == songs_label + 1 {
                    return table::head(Which::Group, s.ui.table.sort(Which::Group), cols);
                }
                let order = lists::order(s);
                let songs = lists::songs(s);
                let j = i - songs_label - 2;
                match order.get(j).and_then(|&k| songs.get(k).map(|x| (k, x))) {
                    Some((k, song)) => row().w_full().px(PAGE_PX - 12.0).py(1.0)
                        .child(table::song_row(s, song, j + 1, cols, format!("song-{k}-{}", song.id), &list, &lists::row_key(s, k))),
                    None if j == 0 && !songs.is_empty() => row().w_full().px(PAGE_PX).child(status("No songs match your filter.", false)),
                    None => col().h(40.0),
                }
            },
        )
        .virtual_rows_version((id.to_string(), m, n_albums, g.n, g.w as u32, cols, sort, query, s.ui.sel.rev, cur_id, s.local_lib.generation));
    let mut page = col().key(key.clone()).grow(1.0).min_h(0.0).w_full();
    if let Some(bar) = filter::bar(s, m, n_songs) {
        page = page.child(el().h(12.0)).child(bar);
    }
    page.child(col().key(format!("{key}-scroll")).grow(1.0).min_h(0.0).w_full().scroll(ScrollSpec { stick_bottom: false }).child(rows))
}

// ---------------------------------------------------------------- Folders

fn name_of(p: &Path) -> String {
    p.file_name().map_or_else(|| p.display().to_string(), |n| n.to_string_lossy().into_owned())
}

/// One folder: icon, name and its song count. Click opens it; right-click plays or queues it.
fn folder_row(key: String, path: &Path, title: String, total: u32) -> El {
    let (p1, p2) = (path.to_path_buf(), path.to_path_buf());
    row().key(key).role("button").label(title.clone()).focusable(true).w_full().min_h(52.0).px(12.0).gap(14.0).center_y()
        .radius(10.0).hover(PAL.row_hover).press(crate::theme::ov(0.04)).cursor(Cursor::Pointer)
        .on_press(move |s: &mut State| s.navigate(Route::Folders { path: Some(p1.clone()) }))
        .on_secondary_press(move |s: &mut State, p, _tx: &mut Tx| ctx_menu::open_folder(s, &p2, [p.x, p.y]))
        .child(el().size(38.0, 38.0).radius(8.0).center().fill(PAL.chip_hover).child(icon(&icons::FOLDER, 18.0).color(PAL.accent)))
        .child(
            col().grow(1.0).min_w(0.0).gap(2.0)
                .child(text(title).font(TRACK_TITLE).color(PAL.text2).truncate())
                .child(text(plural(total as usize, "song", "songs")).font(SMALL13).color(PAL.muted)),
        )
        .child(icon(&icons::CHEVRON_RIGHT, 14.0).color(PAL.dim))
}

/// The music folders (the top of Folders).
fn roots_page(s: &State) -> El {
    let v = &s.local_lib.view;
    let n = s.settings.music_folders.len();
    let sub = if n > 0 { plural(n, "music folder", "music folders") } else { String::new() };
    let page = col().key("folders").grow(1.0).min_h(0.0).w_full().child(head("Folders", sub));
    if s.settings.music_folders.is_empty() || (s.local_lib.songs.is_empty() && !s.local_lib.loaded) {
        return page.child(local_files::placeholder(s).unwrap_or_else(col));
    }
    let mut list = col().w_full().px(PAGE_PX - 12.0).pb(40.0);
    for (i, root) in s.settings.music_folders.iter().enumerate() {
        let total = v.folder(root).map_or(0, |f| f.total);
        list = list.child(folder_row(format!("root-{i}"), root, root.display().to_string(), total));
    }
    page.child(col().key("folders-scroll").grow(1.0).min_h(0.0).w_full().scroll(ScrollSpec { stick_bottom: false }).child(list))
}

/// "Music › Rock › 1970s": where the folder sits, from its music folder. Each part opens.
fn crumbs(s: &State, path: &Path) -> El {
    let mut parts: Vec<PathBuf> = Vec::new();
    if let Some(root) = s.local_lib.view.root_of(path) {
        let mut p = path.parent();
        while let Some(d) = p.filter(|d| d.starts_with(root)) {
            parts.push(d.to_path_buf());
            p = d.parent();
        }
    }
    parts.reverse();
    let mut r = row().key("crumbs").gap(6.0).center_y().min_w(0.0)
        .child(crumb("crumb-top".into(), "Folders".into(), Route::Folders { path: None }));
    for (i, d) in parts.into_iter().enumerate() {
        r = r.child(icon(&icons::CHEVRON_RIGHT, 10.0).color(PAL.dim))
            .child(crumb(format!("crumb-{i}"), name_of(&d), Route::Folders { path: Some(d) }));
    }
    r
}

fn crumb(key: String, label: String, to: Route) -> El {
    row().key(key).role("link").label(label.clone()).focusable(true).min_w(0.0).hover_group().cursor(Cursor::Pointer)
        .on_press(move |s: &mut State| s.navigate(to.clone()))
        .child(text(label).font(SMALL13).color(PAL.muted).group_color(PAL.text).truncate())
}

/// A folder: its subfolders, then its own files as the songs table; Play takes everything below it.
fn folder(s: &State, path: &Path, avail: f32) -> El {
    let key = format!("folder-{}", path.display());
    let v = &s.local_lib.view;
    let Some(f) = v.folder(path) else {
        let msg = if s.local_lib.loaded { "This folder is no longer in your music folders." } else { "Loading\u{2026}" };
        return col().key(key).grow(1.0).min_h(0.0).w_full().child(head(name_of(path), String::new()))
            .child(col().w_full().px(PAGE_PX).child(status(msg, false)));
    };
    let n_dirs = f.dirs.len();
    let n_files = f.files.len();
    let total = f.total as usize;
    let order = lists::order(s);
    let m = order.len();
    let cols = s.ui.table.cols(avail);
    let sort = s.ui.table.sort(Which::Group);
    let query = lists::query(s);
    let list = lists::list_id(s).unwrap_or_default();
    let cur_id = s.playback.song.as_ref().map(|x| x.id.clone());
    // Rows: header, folders…, [table head, files…].
    let files_at = 1 + n_dirs;
    let count = files_at + if n_files > 0 { 1 + m.max(1) } else { 0 };
    let p = path.to_path_buf();
    let rows = col().key("rows").growth(GrowthSpec::default())
        .virtual_rows(
            count,
            54.0,
            |_: &State, i| format!("r{i}").into(),
            move |s: &State, i| {
                let v = &s.local_lib.view;
                let Some(f) = v.folder(&p) else { return col() };
                if i == 0 {
                    let mut sub = Vec::new();
                    if n_dirs > 0 {
                        sub.push(plural(n_dirs, "folder", "folders"));
                    }
                    sub.push(plural(total, "song", "songs"));
                    let p2 = p.clone();
                    let more = ctx_menu::more_button("folder-more", 44.0, move |_s| {
                        ctx_menu::local_items(LocalWhat::Folder(p2.clone()), name_of(&p2))
                    });
                    let play = row().gap(10.0).mt(14.0).center_y()
                        .child(pill("group-play", "Play", &icons::PLAY, PAL.accent, PAL.ink).on_press({
                            let p = p.clone();
                            move |s: &mut State| play_folder(s, &p, false)
                        }))
                        .child(pill("group-shuffle", "Shuffle", &icons::SHUFFLE, PAL.nav_active, PAL.text).on_press({
                            let p = p.clone();
                            move |s: &mut State| play_folder(s, &p, true)
                        }))
                        .child(more);
                    return col().w_full().px(PAGE_PX).pt(20.0).pb(16.0).gap(4.0)
                        .child(crumbs(s, &p))
                        .child(heading(name_of(&p)).min_w(0.0).truncate().mt(4.0))
                        .child(muted(sub.join(" \u{b7} ")))
                        .child(play);
                }
                if i < files_at {
                    let d = &f.dirs[i - 1];
                    let total = v.folder(d).map_or(0, |x| x.total);
                    return row().w_full().px(PAGE_PX - 12.0).py(1.0).child(folder_row(format!("dir-{}", d.display()), d, name_of(d), total));
                }
                if i == files_at {
                    return table::head(Which::Group, s.ui.table.sort(Which::Group), cols).mt(10.0);
                }
                let order = lists::order(s);
                let songs = lists::songs(s);
                let j = i - files_at - 1;
                match order.get(j).and_then(|&k| songs.get(k).map(|x| (k, x))) {
                    Some((k, song)) => row().w_full().px(PAGE_PX - 12.0).py(1.0)
                        .child(table::song_row(s, song, j + 1, cols, format!("song-{k}-{}", song.id), &list, &lists::row_key(s, k))),
                    None if j == 0 && !songs.is_empty() => row().w_full().px(PAGE_PX).child(status("No songs match your filter.", false)),
                    None => col().h(40.0),
                }
            },
        )
        .virtual_rows_version((path.to_path_buf(), m, n_dirs, cols, sort, query, s.ui.sel.rev, cur_id, s.local_lib.generation));
    let mut page = col().key(key.clone()).grow(1.0).min_h(0.0).w_full();
    if let Some(bar) = filter::bar(s, m, n_files) {
        page = page.child(el().h(12.0)).child(bar);
    }
    page.child(col().key(format!("{key}-scroll")).grow(1.0).min_h(0.0).w_full().scroll(ScrollSpec { stick_bottom: false }).child(rows))
}

/// Play or Shuffle on a folder's page: everything in it and below it.
fn play_folder(s: &mut State, path: &Path, shuffle: bool) {
    let songs = s.local_songs(&LocalWhat::Folder(path.to_path_buf()));
    let name = name_of(path);
    if shuffle {
        s.shuffle_play(&songs, &name);
    } else {
        s.play_list(&songs, 0, &name);
    }
}

pub fn folder_page(s: &State, path: Option<&Path>, avail: f32) -> El {
    match path {
        None => roots_page(s),
        Some(p) => folder(s, p, avail),
    }
}
