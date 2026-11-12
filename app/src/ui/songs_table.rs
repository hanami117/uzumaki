//! The sortable "songs" table (Library > Songs, and playlists shown "as Songs"):
//! column model, sort order with a cache, header row and song rows.

use std::cell::RefCell;
use std::cmp::Ordering;
use std::rc::Rc;

use std::cell::Cell;

use rune::{Cursor, El, Spring, TextStyle, col as vstack, el, icon, row, text};

use super::icons;
use super::tracks::{STAR_W, is_current, star_slot};
use super::widgets::{Size, art, fmt_time};
use super::artist_link;
use crate::models::{Artist, Song};
use crate::state::State;
use crate::theme::*;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, serde::Serialize, serde::Deserialize)]
pub enum Col {
    Title,
    Artist,
    Album,
    Genre,
    Year,
    Time,
}

/// Active sort. `col: None` is date added: the list's own order, which is the order songs were
/// added (playlists keep appended songs at the end; the library is fetched sorted by date added).
/// Apple's API sorts by it but never sends the dates, so there is no date column.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct Sort {
    pub col: Option<Col>,
    pub asc: bool,
}

/// The table choices that persist (`Settings::table`): playlists as Songs, and each table's sort.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct TableView {
    pub songs_view: bool,
    pub library: Sort,
    pub playlist: Sort,
    pub local: Sort,
}

impl Default for TableView {
    /// Playlists open as Songs, newest additions on top (the user's preference); the library
    /// by title; local files in folder order.
    fn default() -> Self {
        Self {
            songs_view: true,
            library: Sort { col: Some(Col::Title), asc: true },
            playlist: Sort { col: None, asc: false },
            local: Sort { col: None, asc: true },
        }
    }
}

/// Which table: each remembers its own sort.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Which {
    Library,
    Playlist,
    Local,
    /// A local artist's or folder's songs (local only mode); its sort lasts the session.
    Group,
}

struct Cached {
    col: Option<Col>,
    asc: bool,
    len: usize,
    id: String,
    order: Rc<Vec<usize>>,
}

/// View settings of the tables plus the sorted-order cache. Lives in `UiState`.
pub struct TableState {
    /// Playlists open as the Songs table instead of the artwork list (one global setting).
    pub songs_view: bool,
    pub pl_sort: Sort,
    pub lib_sort: Sort,
    pub local_sort: Sort,
    pub group_sort: Sort,
    cache: [RefCell<Option<Cached>>; 4],
    /// The columns shown at the last layout, for the hysteresis in `Cols::pick`.
    cols: Cell<Cols>,
}

impl Default for TableState {
    fn default() -> Self {
        let v = TableView::default();
        Self { songs_view: v.songs_view, pl_sort: v.playlist, lib_sort: v.library, local_sort: v.local, group_sort: Sort { col: None, asc: true }, cache: Default::default(), cols: Cell::new(Cols::ALL) }
    }
}

impl TableState {
    /// What to save.
    pub fn view(&self) -> TableView {
        TableView { songs_view: self.songs_view, library: self.lib_sort, playlist: self.pl_sort, local: self.local_sort }
    }

    /// Restores a saved view (at startup).
    pub fn apply(&mut self, v: TableView) {
        self.songs_view = v.songs_view;
        self.lib_sort = v.library;
        self.pl_sort = v.playlist;
        self.local_sort = v.local;
    }

    /// The columns to show for a table `avail` points wide (remembers the choice).
    pub fn cols(&self, avail: f32) -> Cols {
        let c = Cols::pick(avail, self.cols.get());
        self.cols.set(c);
        c
    }

    pub fn sort(&self, w: Which) -> Sort {
        match w {
            Which::Library => self.lib_sort,
            Which::Playlist => self.pl_sort,
            Which::Local => self.local_sort,
            Which::Group => self.group_sort,
        }
    }

    /// A header click: a new column sorts ascending, the active one reverses, `None` resets.
    pub fn click(&mut self, w: Which, col: Option<Col>) {
        let sort = match w {
            Which::Library => &mut self.lib_sort,
            Which::Playlist => &mut self.pl_sort,
            Which::Local => &mut self.local_sort,
            Which::Group => &mut self.group_sort,
        };
        *sort = if sort.col == col { Sort { col, asc: !sort.asc } } else { Sort { col, asc: true } };
    }

    /// Indices into `songs` in visible order. Re-sorts only when the sort, the list length or the
    /// list identity (`id`: collection id, or the first song id for the library) changed; every
    /// other call (one per visible row, every second while time ticks) is a cheap lookup.
    pub fn order(&self, w: Which, id: &str, songs: &[Song]) -> Rc<Vec<usize>> {
        let sort = self.sort(w);
        let mut slot = self.cache[w as usize].borrow_mut();
        if let Some(c) = slot.as_ref().filter(|c| c.col == sort.col && c.asc == sort.asc && c.len == songs.len() && c.id == id) {
            return c.order.clone();
        }
        let order = Rc::new(sorted_indices(songs, sort));
        *slot = Some(Cached { col: sort.col, asc: sort.asc, len: songs.len(), id: id.to_string(), order: order.clone() });
        order
    }
}

// ---- sorting ----

/// Lower-case with the common Latin accents removed.
pub fn fold(s: &str) -> String {
    s.trim().chars().flat_map(char::to_lowercase).map(|c| match c {
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' => 'a',
        'ç' => 'c',
        'è' | 'é' | 'ê' | 'ë' => 'e',
        'ì' | 'í' | 'î' | 'ï' => 'i',
        'ñ' => 'n',
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' => 'o',
        'ù' | 'ú' | 'û' | 'ü' => 'u',
        'ý' | 'ÿ' => 'y',
        c => c,
    }).collect()
}

struct Keys {
    title: String,
    artist: String,
    album: String,
    genre: String,
    year: Option<u32>,
    dur: f64,
    track: Option<(u32, u32)>,
}

fn dir(o: Ordering, asc: bool) -> Ordering {
    if asc { o } else { o.reverse() }
}

/// Empty values go last whatever the direction.
fn cmp_str(a: &str, b: &str, asc: bool) -> Ordering {
    match (a.is_empty(), b.is_empty()) {
        (true, true) => Ordering::Equal,
        (true, false) => Ordering::Greater,
        (false, true) => Ordering::Less,
        _ => dir(a.cmp(b), asc),
    }
}

fn cmp_opt<T: Ord>(a: Option<T>, b: Option<T>, asc: bool) -> Ordering {
    match (a, b) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Greater,
        (Some(_), None) => Ordering::Less,
        (Some(a), Some(b)) => dir(a.cmp(&b), asc),
    }
}

fn compare(col: Option<Col>, asc: bool, a: &Keys, b: &Keys) -> Ordering {
    let primary = match col {
        None => Ordering::Equal,
        Some(Col::Title) => cmp_str(&a.title, &b.title, asc),
        Some(Col::Artist) => cmp_str(&a.artist, &b.artist, asc),
        Some(Col::Album) => cmp_str(&a.album, &b.album, asc),
        Some(Col::Genre) => cmp_str(&a.genre, &b.genre, asc),
        Some(Col::Year) => cmp_opt(a.year, b.year, asc),
        Some(Col::Time) => dir(a.dur.total_cmp(&b.dur), asc),
    };
    primary
        .then_with(|| cmp_str(&a.artist, &b.artist, true))
        .then_with(|| cmp_str(&a.album, &b.album, true))
        .then_with(|| cmp_opt(a.track, b.track, true))
}

fn sorted_indices(songs: &[Song], sort: Sort) -> Vec<usize> {
    let mut idx: Vec<usize> = (0..songs.len()).collect();
    if sort.col.is_none() {
        if !sort.asc {
            idx.reverse(); // newest first
        }
        return idx;
    }
    let keys: Vec<Keys> = songs.iter().map(|s| Keys {
        title: fold(&s.title),
        artist: fold(&s.artist),
        album: fold(&s.album),
        genre: s.genre.as_deref().map(fold).unwrap_or_default(),
        year: s.year.as_deref().and_then(|y| y.trim().parse().ok()),
        dur: s.duration,
        track: s.track_number.map(|t| (s.disc_number.unwrap_or(1), t)),
    }).collect();
    // sort_by is stable, so full ties keep the list's own order.
    idx.sort_by(|&a, &b| compare(sort.col, sort.asc, &keys[a], &keys[b]));
    idx
}

// ---- layout ----

/// Which optional columns are shown. Title and Time always are.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Cols {
    pub artist: bool,
    pub num: bool,
    pub album: bool,
    pub year: bool,
    pub genre: bool,
}

const NUM_W: f32 = 60.0; // fits the "ADDED ▲" header
const GENRE_W: f32 = 150.0;
const YEAR_W: f32 = 56.0;
const TIME_W: f32 = 56.0;
/// The last column: the favourite star slot, then the time.
const END_W: f32 = STAR_W + GAP + TIME_W;
const GAP: f32 = 12.0;
const TITLE_MIN: f32 = 220.0;
const FLEX_MIN: f32 = 120.0; // Artist and Album
/// A shown column stays until the width is this far below the width that showed it.
const HYSTERESIS: f32 = 16.0;
/// Column collapse/expand: width and fade on one spring, the same for header and rows.
const COL_MOTION: Spring = Spring::new(0.2, 0.0);

/// Width needed for each optional column to show, in order of importance (last to go first):
/// Artist, number, Album, Year, Genre. Each is the sum of everything ranking above it: Title at
/// its minimum, the star and Time, and every column's own width plus the gap before it.
const NEEDS: [f32; 5] = {
    let base = TITLE_MIN + GAP + END_W; // 328
    let artist = base + GAP + FLEX_MIN; // 460
    let num = artist + GAP + NUM_W; // 532
    let album = num + GAP + FLEX_MIN; // 664
    let year = album + GAP + YEAR_W; // 732
    let genre = year + GAP + GENRE_W; // 894
    [artist, num, album, year, genre]
};

impl Cols {
    #[cfg(test)]
    const NONE: Cols = Cols { artist: false, num: false, album: false, year: false, genre: false };
    pub const ALL: Cols = Cols { artist: true, num: true, album: true, year: true, genre: true };

    fn flags(self) -> [bool; 5] {
        [self.artist, self.num, self.album, self.year, self.genre]
    }

    /// The visible columns for `avail` points, given the previous set (for hysteresis: a column
    /// already shown stays until `avail` drops `HYSTERESIS` below its threshold). A column only
    /// shows when all those ranking above it do.
    pub fn pick(avail: f32, prev: Cols) -> Cols {
        let was = prev.flags();
        let mut f = [false; 5];
        for i in 0..5 {
            let need = if was[i] { NEEDS[i] - HYSTERESIS } else { NEEDS[i] };
            if avail < need {
                break;
            }
            f[i] = true;
        }
        Cols { artist: f[0], num: f[1], album: f[2], year: f[3], genre: f[4] }
    }
}

/// One column's box, shown or collapsed. The outer box animates its width (and fades) on
/// `COL_MOTION` and clips; the inner content keeps its own size, with the gap before it, so
/// nothing re-wraps while the box opens or closes.
fn cell(col: Col, shown: bool, inner: El) -> El {
    let outer = row().center_y().clip_rounded(0.0).motion(COL_MOTION).opacity_motion(COL_MOTION)
        .opacity(if shown { 1.0 } else { 0.0 });
    let fixed = |outer: El, w: f32, inner: El| outer.w(if shown { w + GAP } else { 0.0 }).min_w(0.0).child(inner.w(w).min_w(w).ml(GAP));
    match col {
        Col::Title => outer.grow(1.3).min_w(TITLE_MIN).child(inner.w_full()),
        Col::Artist | Col::Album => {
            let o = outer.min_w(0.0);
            let o = if shown { o.grow(1.0) } else { o.grow(0.0).w(0.0) };
            o.child(row().w_full().pl(GAP).center_y().child(inner.w_full()))
        }
        Col::Genre => fixed(outer, GENRE_W, inner),
        Col::Year => fixed(outer, YEAR_W, inner),
        Col::Time => outer.w(END_W + GAP).child(row().w(END_W).ml(GAP).justify_end().child(inner)),
    }
}

/// The number column's box (before Title, so the gap trails).
fn num_cell(shown: bool, inner: El) -> El {
    row().center_y().clip_rounded(0.0).motion(COL_MOTION).opacity_motion(COL_MOTION)
        .opacity(if shown { 1.0 } else { 0.0 }).w(if shown { NUM_W + GAP } else { 0.0 }).min_w(0.0)
        .child(inner.w(NUM_W))
}

fn is_shown(cols: Cols, col: Col) -> bool {
    match col {
        Col::Title | Col::Time => true,
        Col::Artist => cols.artist,
        Col::Album => cols.album,
        Col::Genre => cols.genre,
        Col::Year => cols.year,
    }
}

/// All six columns in display order, with their header labels (the number column is separate).
const COLUMNS: [(Col, &str); 6] = [
    (Col::Title, "Title"),
    (Col::Artist, "Artist"),
    (Col::Album, "Album"),
    (Col::Genre, "Genre"),
    (Col::Year, "Year"),
    (Col::Time, "Time"),
];

fn head_cell(w: Which, sort: Sort, col: Option<Col>, label: &'static str, base: El, shown: bool) -> El {
    let active = sort.col == col;
    let mut cell = base.key(format!("h-{}", label.to_lowercase())).role("button").label(label).focusable(shown).h(32.0)
        .gap(5.0).cursor(Cursor::Pointer).hover(crate::theme::ov(0.05)).radius(6.0)
        .on_press_tx(move |s: &mut State, tx: &mut rune::Tx| {
            s.ui.table.click(w, col);
            s.remember_table();
            tx.instant();
        })
        .child(text(label.to_uppercase()).font(TABLE_HEAD).color(if active { PAL.text2 } else { PAL.dim }));
    if active {
        cell = cell.child(icon(if sort.asc { &icons::SORT_UP } else { &icons::SORT_DOWN }, 10.0).color(PAL.accent));
    }
    cell
}

/// Column headers. "Added" sorts by date added (oldest first, again for newest first).
pub fn head(w: Which, sort: Sort, cols: Cols) -> El {
    let num = head_cell(w, sort, None, if matches!(w, Which::Local | Which::Group) { "#" } else { "Added" }, row().center_y(), cols.num);
    let mut r = row().w_full().min_h(32.0).px(12.0).center_y().mb(6.0).animate_layout().child(num_cell(cols.num, num));
    for (col, label) in COLUMNS {
        let shown = is_shown(cols, col);
        let base = match col {
            Col::Time => row().center_y().justify_end(),
            _ => row().center_y(),
        };
        r = r.child(cell(col, shown, head_cell(w, sort, Some(col), label, base, shown)));
    }
    row().w_full().px(super::PAGE_PX - 12.0).child(r)
}

/// One table row of list `list`. `n` is the 1-based position, `rk` the row's key in the list
/// (`lists::row_key`).
pub fn song_row(s: &State, song: &Song, n: usize, cols: Cols, key: String, list: &str, rk: &str) -> El {
    if song.is_local() && s.local_only() {
        // Local only mode: a file's artist name opens that artist of the music folders.
        return song_row_with(s, song, n, cols, key, list, rk, &[crate::local::view::artist_of(song)]);
    }
    song_row_with(s, song, n, cols, key, list, rk, &song.artists)
}

/// `song_row` whose artist name links to `artists` (none: plain text).
#[allow(clippy::too_many_arguments)]
pub fn song_row_with(s: &State, song: &Song, n: usize, cols: Cols, key: String, list: &str, rk: &str, artists: &[Artist]) -> El {
    let cur = is_current(s, song);
    let dim = |t: &str| text(t.to_string()).font(BODY14).color(PAL.muted).truncate().min_w(0.0);
    let num = el().key(format!("{key}-play")).w(NUM_W).h(36.0).items_center()
        .child(text(n.to_string()).font(TIME14).color(if cur { PAL.accent } else { PAL.dim }).group_opacity(1.0, 0.0))
        .child(el().absolute(0.0, 10.0).group_opacity(0.0, 1.0).child(icon(&icons::PLAY, 16.0).color(PAL.text)));
    let num = super::select::play_button(num, list, rk);
    let mut star = Some(star_slot(s, song, &key));
    let row_key = key.clone();
    let mut r = row().key(key).role("button").label(song.title.clone()).focusable(true).w_full().min_h(48.0).px(12.0)
        .center_y().radius(8.0).animate_layout().hover(PAL.row_hover).hover_group().cursor(Cursor::Pointer).child(num_cell(cols.num, num));
    for (col, _) in COLUMNS {
        let shown = is_shown(cols, col);
        let inner = match col {
            // With the Artist column gone the artist sits under the title, so it never vanishes.
            Col::Title => row().w_full().gap(GAP).center_y()
                .child(art(s, song.artwork.as_ref(), &song.album, Size::Fixed(36.0), 6.0, 0.0))
                .child(vstack().grow(1.0).min_w(0.0).shrink()
                    .child(text(song.title.clone()).font(TRACK_TITLE).color(if cur { PAL.accent } else { PAL.text2 }).truncate().min_w(0.0))
                    .child(row().h(if cols.artist { 0.0 } else { 16.0 }).clip_rounded(0.0).motion(COL_MOTION)
                        .opacity_motion(COL_MOTION).opacity(if cols.artist { 0.0 } else { 1.0 })
                        .child(artist_link::link(
                            format!("{row_key}-subartist"),
                            text(song.artist.clone()).font(TextStyle::new(12.0, 400.0)).color(PAL.muted).truncate().min_w(0.0),
                            artists,
                            PAL.text,
                        )))),
            Col::Artist => artist_link::link(format!("{row_key}-artist"), dim(&song.artist), artists, PAL.text),
            Col::Album => dim(&song.album),
            Col::Genre => dim(song.genre.as_deref().unwrap_or("")),
            Col::Year => dim(song.year.as_deref().unwrap_or("")),
            // The time has a fixed box so the star never shifts with its width.
            Col::Time => row().gap(GAP).center_y().child(star.take().unwrap_or_else(el))
                .child(row().w(TIME_W).justify_end().child(text(fmt_time(song.duration)).font(TIME14).color(PAL.muted))),
        };
        r = r.child(cell(col, shown, inner));
    }
    super::select::wire(s, r, rk, list)
}

/// Small "Playlist | Songs" segmented control.
pub fn segmented(songs_view: bool) -> El {
    let seg = |key: &'static str, label: &'static str, on: bool, songs: bool| {
        el().key(key).role("button").label(label).focusable(true).h(30.0).px(14.0).radius(15.0).center().cursor(Cursor::Pointer)
            .fill(if on { crate::theme::seg_on() } else { rune::Color::TRANSPARENT })
            .hover(crate::theme::ov(if on { 0.0 } else { 0.06 }))
            .on_press_tx(move |s: &mut State, tx: &mut rune::Tx| {
                s.ui.table.songs_view = songs;
                s.remember_table();
                tx.instant();
            })
            .child(text(label).font(TextStyle::new(13.0, 600.0)).color(if on { PAL.text } else { PAL.muted }))
    };
    row().padding(3.0).radius(18.0).fill(PAL.chip).items_center()
        .child(seg("view-playlist", "Playlist", !songs_view, false))
        .child(seg("view-songs", "Songs", songs_view, true))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(c: Cols) -> usize {
        c.flags().iter().filter(|&&b| b).count()
    }

    /// The set at `w` after reaching it from `from` in steps of 1 pt (as a resize would).
    fn walk(from: f32, to: f32) -> Cols {
        let mut c = Cols::pick(from, Cols::NONE);
        let step = if to > from { 1.0 } else { -1.0 };
        let mut w = from;
        while (w - to).abs() > 0.5 {
            w += step;
            c = Cols::pick(w, c);
        }
        c
    }

    #[test]
    fn priority_order_when_narrowing() {
        let none = Cols::NONE;
        assert_eq!(Cols::pick(1400.0, Cols::ALL), Cols::ALL);
        assert_eq!(Cols::pick(300.0, Cols::ALL), none);
        // Narrowing drops Genre, Year, Album, number, Artist, in that order.
        let mut c = Cols::ALL;
        let mut order = vec![];
        let mut w = 1000.0;
        while w >= 280.0 {
            let next = Cols::pick(w, c);
            for (name, was, now) in [
                ("genre", c.genre, next.genre),
                ("year", c.year, next.year),
                ("album", c.album, next.album),
                ("num", c.num, next.num),
                ("artist", c.artist, next.artist),
            ] {
                if was && !now {
                    order.push(name);
                }
            }
            c = next;
            w -= 1.0;
        }
        assert_eq!(order, ["genre", "year", "album", "num", "artist"]);
    }

    #[test]
    fn thresholds_and_widening() {
        for (i, need) in NEEDS.iter().enumerate() {
            let prev = Cols::pick(*need - 1.0, Cols::NONE);
            assert_eq!(n(prev), i, "just under {need}");
            assert_eq!(n(Cols::pick(*need, prev)), i + 1, "at {need}");
        }
        assert_eq!(NEEDS, [460.0, 532.0, 664.0, 732.0, 894.0]);
        // Everything ranking above fits: a wide window shows all, never a lower rank alone.
        for w in 280..1400 {
            let f = Cols::pick(w as f32, Cols::ALL).flags();
            assert!(f.windows(2).all(|p| p[0] || !p[1]));
        }
    }

    #[test]
    fn hysteresis_stops_flicker() {
        // Widening to just under the threshold keeps it hidden; narrowing from above keeps it shown.
        let need = NEEDS[4]; // genre
        assert!(walk(800.0, need + 1.0).genre);
        assert!(!walk(800.0, need - 1.0).genre);
        assert!(walk(1000.0, need - HYSTERESIS + 1.0).genre);
        assert!(!walk(1000.0, need - HYSTERESIS - 1.0).genre);
        // Wobbling inside the band changes nothing.
        let mut c = walk(1000.0, need);
        let before = c;
        for w in [need - 5.0, need + 3.0, need - 12.0, need - 1.0] {
            c = Cols::pick(w, c);
            assert_eq!(c, before);
        }
    }
}
