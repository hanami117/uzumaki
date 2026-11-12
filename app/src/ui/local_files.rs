//! Library → Local Files: the songs in the music folders, as the sortable songs table.

use rune::{El, GrowthSpec, ScrollSpec, Tx, col, row, text};

use super::songs_table::{self as table, Which};
use super::widgets::{heading, pill, small_button};
use super::{PAGE_PX, filter, icons, lists, settings};
use crate::state::State;
use crate::theme::*;

/// 1234 -> "1,234".
pub fn thousands(n: usize) -> String {
    let d = n.to_string();
    let mut out = String::new();
    for (i, c) in d.chars().enumerate() {
        if i > 0 && (d.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// Opens the folder picker; the chosen folder is added and scanned.
pub fn pick_folder(tx: &mut Tx) {
    tx.open_folder(
        rune::FileDialogOptions { title: "Add a music folder".into(), ..Default::default() },
        |s: &mut State, result: rune::FileDialogResult, _tx: &mut Tx| {
            match result {
                Ok(Some(path)) => s.add_music_folder(path),
                Ok(None) => {} // cancelled
                Err(e) => s.toast = Some(format!("Couldn't open the folder picker: {e}")),
            }
        },
    );
}

fn subtitle(s: &State) -> String {
    let lib = &s.local_lib;
    let n = lib.songs.len();
    let mut t = format!("{} song{}", thousands(n), if n == 1 { "" } else { "s" });
    if lib.scanning {
        t.push_str(&format!(" \u{b7} Scanning\u{2026} {} files", thousands(lib.scanned)));
    }
    t
}

fn play(s: &mut State, shuffle: bool) {
    let songs = lists::visible(s);
    if songs.is_empty() {
        return;
    }
    if shuffle {
        s.shuffle_play(&songs, "Local Files");
    } else {
        s.play_list(&songs, 0, "Local Files");
    }
}

pub fn empty(title: &str, body: &str, button: Option<El>) -> El {
    let mut c = col().key("local-empty").w_full().px(PAGE_PX).pt(48.0).gap(10.0).items_start()
        .child(text(title.to_string()).font(H2).color(PAL.text))
        .child(text(body.to_string()).font(BODY14).color(PAL.dim).wrap().max_w(520.0));
    if let Some(b) = button {
        c = c.child(row().mt(8.0).child(b));
    }
    c
}

fn songs(s: &State, avail: f32) -> El {
    let order = lists::order(s);
    let count = order.len();
    let sort = s.ui.table.local_sort;
    let cols = s.ui.table.cols(avail);
    let first = s.local_lib.songs.first().map(|x| x.id.clone()).unwrap_or_default();
    let query = lists::query(s);
    let cur_id = s.playback.song.as_ref().map(|x| x.id.clone());
    let rows = col().key("local-songs").grow(1.0).min_h(0.0).w_full().scroll(ScrollSpec { stick_bottom: false }).child(
        col().key("rows").growth(GrowthSpec::default())
            .virtual_rows(
                count.max(1),
                50.0,
                |_: &State, i| format!("r{i}").into(),
                move |s: &State, i| {
                    let order = lists::order(s);
                    match order.get(i).and_then(|&k| s.local_lib.songs.get(k).map(|x| (k, x))) {
                        Some((k, song)) => row().w_full().px(PAGE_PX - 12.0).py(1.0).child(table::song_row(
                            s,
                            song,
                            i + 1,
                            cols,
                            format!("song-{i}-{}", song.id),
                            "local-files",
                            &lists::row_key(s, k),
                        )),
                        None => row().w_full().px(PAGE_PX).child(super::widgets::status("No songs match your filter.", false)),
                    }
                },
            )
            .virtual_rows_version((count, sort, cols, first, query, s.ui.sel.rev, cur_id, s.local_lib.songs.len())),
    );
    col().w_full().grow(1.0).min_h(0.0).child(table::head(Which::Local, sort, cols)).child(rows)
}

/// What the local pages show instead of their content while there is none: no music folder
/// yet, a first scan, or folders without music. None once there are songs.
pub fn placeholder(s: &State) -> Option<El> {
    let lib = &s.local_lib;
    if !lib.songs.is_empty() {
        return None;
    }
    Some(if s.settings.music_folders.is_empty() {
        empty(
            "Play your own music files",
            if s.local_only() {
                "Add a folder and Uzumaki plays the FLAC, MP3, M4A, Ogg and WAV files in it."
            } else {
                "Add a folder and Uzumaki plays the FLAC, MP3, M4A, Ogg and WAV files in it next to your Apple Music library."
            },
            Some(small_button("local-add", "Add a music folder\u{2026}").on_press_tx(|_s: &mut State, tx: &mut Tx| pick_folder(tx))),
        )
    } else if lib.scanning || !lib.loaded {
        empty("Scanning your music folders\u{2026}", &format!("{} files looked at so far.", thousands(lib.scanned)), None)
    } else {
        empty(
            "No music found in your folders",
            "Uzumaki looked for FLAC, MP3, M4A, Ogg and WAV files. Check the folders in Settings.",
            Some(small_button("local-settings", "Settings").on_press(|s: &mut State| settings::open_tab(s, settings::Tab::Local))),
        )
    })
}

pub fn page(s: &State, avail: f32) -> El {
    let lib = &s.local_lib;
    let has_songs = !lib.songs.is_empty();
    // Local only mode folds Local Files into its Songs tab.
    let title = if s.local_only() { "Songs" } else { "Local Files" };
    let mut head = col().w_full().px(PAGE_PX).pt(28.0).pb(16.0).gap(4.0).child(heading(title));
    if !s.settings.music_folders.is_empty() {
        head = head.child(text(subtitle(s)).font(SMALL13).color(PAL.muted));
    }
    if has_songs {
        head = head.child(
            row().gap(10.0).mt(14.0).child(pill("local-play", "Play", &icons::PLAY, PAL.accent, PAL.ink).on_press(|s: &mut State| play(s, false)))
                .child(pill("local-shuffle", "Shuffle", &icons::SHUFFLE, PAL.nav_active, PAL.text).on_press(|s: &mut State| play(s, true))),
        );
    }

    let body = placeholder(s).unwrap_or_else(|| songs(s, avail));

    let mut page = col().key("local-files").grow(1.0).min_h(0.0).w_full().child(head);
    if has_songs {
        page = page.child(filter::bar(s, lists::order(s).len(), lib.songs.len()).unwrap_or_else(col));
    }
    page.child(body)
}
