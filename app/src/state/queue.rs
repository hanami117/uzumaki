//! The play order. The app owns it; the bridge only plays it lazily (see "queue" in
//! bridge/preload.js), so every change here is followed by sending `upcoming()` to the bridge.
//!
//! Two parts, as in Apple Music: songs the user added (Play Next / Add to Queue), which play
//! first, then the rest of the list playback was started from ("context"), shuffled or not.

use std::collections::VecDeque;

use crate::models::Song;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum Repeat {
    #[default]
    Off,
    All,
    One,
}

impl Repeat {
    /// Off → All → One → Off, like the player bar button.
    pub fn next(self) -> Self {
        match self {
            Repeat::Off => Repeat::All,
            Repeat::All => Repeat::One,
            Repeat::One => Repeat::Off,
        }
    }

    pub fn bridge_name(self) -> &'static str {
        match self {
            Repeat::Off => "off",
            Repeat::All => "all",
            Repeat::One => "one",
        }
    }
}

/// Session-wide confirmed playback, independent of queue replacement. Oldest first.
#[derive(Clone, Debug, Default)]
pub struct History {
    pub tracks: Vec<(u64, Song)>,
    heard: Option<Song>,
    serial: u64,
}
impl History {
    /// A History saved by the last run (`resume.rs`), oldest first. The current song isn't heard
    /// yet: it joins the History once another one plays.
    pub fn restore(songs: Vec<Song>) -> Self {
        let mut h = History::default();
        for s in songs.into_iter().rev().take(50).collect::<Vec<_>>().into_iter().rev() {
            h.serial += 1;
            h.tracks.push((h.serial, s));
        }
        h
    }
    pub fn heard(&mut self, song: &Song) {
        if self.heard.as_ref().is_some_and(|s| s.same_track(song)) { return; }
        if let Some(previous) = self.heard.replace(song.clone()) {
            self.serial += 1;
            self.tracks.push((self.serial, previous));
        }
        // The current track is never also presented as history (including id aliases).
        self.tracks.retain(|(_, s)| !s.same_track(song));
        if self.tracks.len() > 50 { self.tracks.drain(..self.tracks.len() - 50); }
    }
}

/// How many played songs a station session remembers.
pub const STATION_HISTORY: usize = 50;

/// A radio station being played. Apple picks its songs one at a time as you listen, so there is
/// no Up Next to show; what the app can show is the songs already heard in this session.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StationRun {
    pub id: String,
    pub name: String,
    /// The song the station is playing now (or the last one it played).
    pub heard: Option<Song>,
    /// Songs already played, most recent first, without the current one, at most `STATION_HISTORY`.
    pub played: Vec<Song>,
    /// A played song was clicked and plays on its own; the station picks up again when it (and
    /// the songs queued by hand) end. MusicKit can't queue a station behind a song, so the app does.
    pub one_off: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PlayQueue {
    /// The list playback started from, in its own order (to undo shuffle).
    pub original: Vec<Song>,
    /// That list in play order (shuffled when shuffle is on).
    pub context: Vec<Song>,
    /// Index in `context` of the current song, or of the one after which `user` songs play.
    pub pos: usize,
    /// Index in `context` where MusicKit's own queue starts: Previous before it must restart
    /// the queue one song earlier (MusicKit doesn't have those songs).
    pub base: usize,
    /// Play Next / Add to Queue songs, played before the context continues.
    pub user: VecDeque<Song>,
    /// Name of the list, for "Continue playing from …".
    pub source: String,
    /// Whether `context` is a shuffled order (so a new lap gets a fresh shuffle).
    pub shuffled: bool,
    /// The next lap's order under shuffle + repeat all, decided as soon as the current lap is
    /// known (and again after every edit of it), so what Up Next shows after "Repeat" is exactly
    /// what will play, and the bridge (`cycle_ids`) and the app (`peek_next`, `advance`,
    /// `advance_to`) agree on it. Promoted to `context` at the wrap. Only valid while `lap()` says so.
    pub next_lap: Option<Vec<Song>>,
    /// Repeat all is on (as of the last `plan_lap`): a next lap exists and Up Next shows it.
    pub looping: bool,
    /// Bumps whenever the next lap is planned, dropped or promoted: the key of what Up Next draws.
    pub lap_gen: u64,
    /// Set while a radio station plays (or a one-off song from its history): `context` is empty
    /// then, and `user` still holds what was queued by hand.
    pub station: Option<StationRun>,
    /// The song taken off `user` that is playing now. It is not in `context` at `pos`, so when
    /// MusicKit reports it (again), `advance_to` must not go looking for a copy of it in the list
    /// (Play Next of a song that is also further down the list used to move the position there and
    /// drop the songs in between). Cleared whenever playback lands on any other song.
    pub user_playing: Option<Song>,
}

impl PlayQueue {
    /// A queue for a radio station that has just been asked to play. `keep` carries the history
    /// of the same session over (the station picking up again after a one-off song).
    pub fn for_station(id: &str, name: &str, keep: Option<StationRun>) -> Self {
        let run = StationRun { id: id.into(), name: name.into(), one_off: false, ..keep.unwrap_or_default() };
        Self { source: name.into(), station: Some(run), ..Self::default() }
    }

    /// The station says (via MusicKit) that `song` plays now. The song that played before it goes
    /// to the history, newest first; a song never shows twice, and not while it is the current one.
    pub fn station_heard(&mut self, song: &Song) {
        let Some(run) = &mut self.station else { return };
        if run.heard.as_ref().is_some_and(|h| h.same_track(song)) {
            return;
        }
        if let Some(prev) = run.heard.replace(song.clone()) {
            run.played.retain(|p| !p.same_track(&prev));
            run.played.insert(0, prev);
        }
        run.played.retain(|p| !p.same_track(song));
        run.played.truncate(STATION_HISTORY);
    }

    /// Songs the station already played, most recent first (empty without a station).
    pub fn station_played(&self) -> &[Song] {
        self.station.as_ref().map_or(&[], |r| r.played.as_slice())
    }

    fn one_off(&self) -> bool {
        self.station.as_ref().is_some_and(|r| r.one_off)
    }

    /// The station itself picks the songs now (not a one-off from its history or a queued song).
    /// MusicKit's queue is then the station, which never runs out: songs queued by hand are
    /// never given to it, the app plays them itself (`station_take_user`).
    pub fn station_live(&self) -> bool {
        self.station.as_ref().is_some_and(|r| !r.one_off)
    }

    /// Plays history entry `i` on its own: it becomes the whole context, the station resumes
    /// after it. Songs queued by hand stay and play after it.
    fn replay_from_station(&mut self, i: usize) -> Option<Song> {
        let song = self.station.as_ref()?.played.get(i).cloned()?;
        if !song.playable {
            return None;
        }
        self.play_alone(song)
    }

    /// While the station picks the songs: takes the first song queued by hand (Play Next / Add
    /// to Queue) and plays it as a one-off, like a song replayed from the history. The other
    /// queued songs follow it in MusicKit's queue, and when they are done the station picks up
    /// again (`resume_station`). Unavailable songs are dropped. None without a live station or
    /// with nothing queued.
    pub fn station_take_user(&mut self) -> Option<Song> {
        if !self.station_live() {
            return None;
        }
        while let Some(song) = self.user.pop_front() {
            if song.playable {
                let song = self.play_alone(song)?;
                self.user_playing = Some(song.clone());
                return Some(song);
            }
        }
        None
    }

    /// MusicKit reports `song` while the station plays and songs are queued by hand: true when
    /// it is a new pick of the station (so the app missed the end of the previous song and the
    /// queued song must take over at once). Not for the station's first song, the song already
    /// playing, or the queued song itself.
    pub fn station_skipped_queue(&self, song: &Song) -> bool {
        let Some(run) = self.station.as_ref().filter(|r| !r.one_off) else { return false };
        let Some(front) = self.user.front() else { return false };
        !front.same_track(song) && run.heard.as_ref().is_some_and(|h| !h.same_track(song))
    }

    /// `song` becomes the whole context of a station session (one-off): no lap, the station
    /// resumes when it and the songs queued after it end.
    fn play_alone(&mut self, song: Song) -> Option<Song> {
        self.user_playing = None;
        self.station_heard(&song);
        self.station.as_mut()?.one_off = true;
        self.original = vec![song.clone()];
        self.context = vec![song.clone()];
        self.pos = 0;
        self.base = 0;
        self.drop_lap();
        self.looping = false;
        Some(song)
    }

    /// Starts at `list[index]`. With shuffle, the rest of the list follows in random order.
    pub fn start(list: &[Song], index: usize, shuffle: bool, source: &str, user: VecDeque<Song>) -> Self {
        let original: Vec<Song> = list.to_vec();
        let (context, pos) = if shuffle {
            let mut rest: Vec<Song> = list.iter().enumerate().filter(|(i, _)| *i != index).map(|(_, s)| s.clone()).collect();
            shuffle_in_place(&mut rest);
            let mut c = Vec::with_capacity(list.len());
            c.push(list[index].clone());
            c.extend(rest);
            (c, 0)
        } else {
            (original.clone(), index)
        };
        Self { original, context, pos, base: pos, user, source: source.to_string(), shuffled: shuffle, next_lap: None, looping: false, lap_gen: 0, station: None, user_playing: None }
    }

    pub fn current(&self) -> Option<&Song> {
        self.context.get(self.pos)
    }

    /// What follows the current song, in order: user songs, then the rest of the context.
    pub fn up_next(&self) -> impl Iterator<Item = &Song> {
        self.user.iter().chain(self.context.iter().skip(self.pos + 1))
    }

    /// How many entries `up_next()` has.
    pub fn up_next_len(&self) -> usize {
        self.user.len() + self.context.len().saturating_sub(self.pos + 1)
    }

    /// The lap after this one, shown under "Repeat" (repeat all only; empty otherwise): the
    /// planned shuffle, or the list order without shuffle.
    pub fn next_lap_songs(&self) -> &[Song] {
        if !self.looping {
            return &[];
        }
        if self.shuffled { self.lap().map_or(&[], |l| l.as_slice()) } else { &self.context }
    }

    /// Ids for the bridge: what follows the current song (playable songs only), up to the next
    /// local file. MusicKit's queue then runs out there, and the app plays the file itself.
    pub fn upcoming_ids(&self) -> Vec<String> {
        if self.station_live() {
            return Vec::new(); // MusicKit's queue is the station; queued songs are the app's
        }
        self.up_next().take_while(|s| !s.is_local()).filter(|s| s.playable).map(|s| s.play_id().to_string()).collect()
    }

    /// The next lap, for repeat all, when the bridge can loop into it on its own (no local
    /// files in it; with them the app wraps around itself, see `advance`): the planned shuffle,
    /// else the context's order.
    pub fn cycle_ids(&self) -> Option<Vec<String>> {
        if self.station.is_some() || self.context.iter().any(Song::is_local) {
            return None;
        }
        Some(self.lap().unwrap_or(&self.context).iter().filter(|s| s.playable).map(|s| s.play_id().to_string()).collect())
    }

    /// The song after the current one, without moving: the first user song, else the next of
    /// the context (wrapping to the start with repeat all).
    pub fn peek_next(&self, repeat: Repeat) -> Option<&Song> {
        if let Some(u) = self.user.front() {
            return Some(u);
        }
        match self.context.get(self.pos + 1) {
            Some(s) => Some(s),
            None if repeat == Repeat::All && !self.one_off() => self.lap().unwrap_or(&self.context).first(),
            None => None,
        }
    }

    /// The planned next lap, if it still fits the context (same size; edits drop it).
    fn lap(&self) -> Option<&Vec<Song>> {
        self.next_lap.as_ref().filter(|l| l.len() == self.context.len())
    }

    /// The current song is the last of the lap, so the next one is the wrap.
    fn at_end(&self) -> bool {
        self.pos + 1 >= self.context.len()
    }

    /// Under shuffle + repeat all: decides the next lap's order now (a fresh shuffle that doesn't
    /// open with the last two songs of this lap), and keeps it until an edit drops it. Without
    /// them it drops a stale plan; without shuffle the next lap is the list order, nothing to
    /// store. Returns whether the plan changed, so the caller re-sends it to the bridge.
    pub fn plan_lap(&mut self, repeat: Repeat) -> bool {
        self.looping = repeat == Repeat::All && !self.context.is_empty() && !self.one_off();
        if !(self.looping && self.shuffled) {
            return self.drop_lap();
        }
        if self.lap().is_some() {
            return false;
        }
        let mut lap = self.context.clone();
        shuffle_in_place(&mut lap);
        // The lap never opens with the song that just ended the last one, nor with the one before
        // it: Previous on the last song lands there, and `advance_to` must not take it for the wrap.
        let n = lap.len();
        let avoid = |s: &Song| s.same_track(&self.context[n - 1]) || (n > 2 && s.same_track(&self.context[n - 2]));
        if n > 1 && avoid(&lap[0]) {
            if let Some(j) = (1..n).find(|&j| !avoid(&lap[j])) {
                lap.swap(0, j);
            }
        }
        self.next_lap = Some(lap);
        self.lap_gen += 1;
        true
    }

    fn drop_lap(&mut self) -> bool {
        let had = self.next_lap.take().is_some();
        if had {
            self.lap_gen += 1;
        }
        had
    }

    /// The wrap: the planned lap (or the same order) becomes the context, and the lap after it
    /// is planned at once.
    fn promote_lap(&mut self) {
        if let Some(l) = self.next_lap.take() {
            self.context = l;
        }
        self.pos = 0;
        self.base = 0;
        self.lap_gen += 1;
        if self.looping {
            self.plan_lap(Repeat::All);
        }
    }

    /// Whether MusicKit was given the song after the current one, so it moves on by itself:
    /// false when that song is a local file, or the wrap of a list MusicKit can't loop.
    pub fn bridge_has_next(&self, repeat: Repeat) -> bool {
        if self.station_live() && !self.user.is_empty() {
            return false; // the station would pick its own song over the queued one
        }
        match self.peek_next(repeat) {
            None => false,
            Some(s) if s.is_local() => false,
            Some(_) => {
                let wraps = self.user.is_empty() && self.pos + 1 >= self.context.len();
                !(wraps && self.cycle_ids().is_none())
            }
        }
    }

    /// Moves to the song after the current one and returns it (see `peek_next`). Used where
    /// the app, not MusicKit, decides what plays next: after a local file, and at the edge
    /// between an Apple run and a local file. MusicKit's queue starts again from the new song,
    /// so `base` moves with it.
    pub fn advance(&mut self, repeat: Repeat) -> Option<Song> {
        if self.station_live() {
            return self.station_take_user();
        }
        if let Some(u) = self.user.pop_front() {
            self.user_playing = Some(u.clone());
            return Some(u);
        }
        self.user_playing = None;
        let next = if self.pos + 1 < self.context.len() {
            self.pos + 1
        } else if repeat == Repeat::All && !self.context.is_empty() && !self.one_off() {
            self.plan_lap(repeat);
            self.promote_lap();
            return self.current().cloned();
        } else {
            return None;
        };
        self.pos = next;
        self.base = next;
        self.current().cloned()
    }

    /// MusicKit says `song` is playing now: move our position to it. Returns false when the song
    /// isn't one we queued (e.g. repeat one replaying, or playback started elsewhere).
    pub fn advance_to(&mut self, song: &Song) -> bool {
        if self.user.front().is_some_and(|u| u.same_track(song)) {
            self.user_playing = self.user.pop_front();
            return true;
        }
        // The queued song already taken off `user` (a duplicate report, or `jump`/`advance` got
        // there first): it stays where it is, whatever copies the list holds.
        if self.user_playing.as_ref().is_some_and(|u| u.same_track(song)) {
            return true;
        }
        self.user_playing = None;
        if self.current().is_some_and(|c| c.same_track(song)) {
            return true;
        }
        // MusicKit looped into the planned next lap: its first song is the wrap.
        if self.at_end() && self.lap().is_some_and(|l| l[0].same_track(song)) {
            self.promote_lap();
            return true;
        }
        let n = self.context.len();
        // Forward first (normal play), then from the start (repeat all wrapped, or Previous).
        let found = (self.pos + 1..n).chain(0..self.pos).find(|&i| self.context[i].same_track(song));
        match found {
            Some(i) => {
                if i < self.base {
                    self.base = i;
                }
                self.pos = i;
                true
            }
            None => false,
        }
    }

    /// Shuffle turned on or off mid-list: reorder what hasn't played yet. The current song stays.
    pub fn set_shuffle(&mut self, on: bool) {
        self.shuffled = on;
        self.drop_lap();
        let Some(current) = self.current().cloned() else { return };
        let played = &self.context[..=self.pos];
        let mut rest: Vec<Song> = if on {
            let mut r = self.context[self.pos + 1..].to_vec();
            shuffle_in_place(&mut r);
            r
        } else {
            // Back to list order, continuing after the current song's place in the list.
            let k = self.original.iter().position(|s| s.same_track(&current)).unwrap_or(0);
            self.original[k + 1..].to_vec()
        };
        let mut context = played.to_vec();
        context.append(&mut rest);
        self.context = context;
    }

    pub fn play_next(&mut self, songs: &[Song]) {
        for s in songs.iter().rev() {
            self.user.push_front(s.clone());
        }
    }

    pub fn add_to_queue(&mut self, songs: &[Song]) {
        self.user.extend(songs.iter().cloned());
    }

    /// Local only mode: Apple songs leave the queue (they can't play without the bridge); the
    /// files stay in order, and the current one keeps its place. Stations are Apple's.
    pub fn keep_local(&mut self) {
        if self.station.take().is_some() {
            self.context.clear();
            self.original.clear();
            self.pos = 0;
        }
        let before = self.context.iter().take(self.pos).filter(|s| !s.is_local()).count();
        self.pos = self.pos.saturating_sub(before);
        self.context.retain(Song::is_local);
        self.original.retain(Song::is_local);
        self.user.retain(Song::is_local);
        self.pos = self.pos.min(self.context.len().saturating_sub(1));
        self.base = self.pos;
        self.drop_lap();
        self.lap_gen += 1;
    }

    /// Jumps to the `i`-th song of `up_next()`: songs before it in the user part are dropped.
    /// Numbers past it count the next lap, then the station's played songs (see `replay_from_station`).
    /// Returns the song to start.
    pub fn jump(&mut self, i: usize) -> Option<Song> {
        if i < self.user.len() {
            self.user.drain(..i);
            if self.station_live() {
                return self.station_take_user(); // a one-off: the station resumes after the queue
            }
            self.user_playing = self.user.pop_front();
            return self.user_playing.clone();
        }
        self.user_playing = None;
        let rest = self.context.len().saturating_sub(self.pos + 1);
        let k = i - self.user.len();
        if k >= rest {
            // Past this lap: the next lap, then (on a station) the songs it already played.
            let lap = self.next_lap_songs().len();
            return if k - rest < lap { self.jump_into_next_lap(k - rest) } else { self.replay_from_station(k - rest - lap) };
        }
        let j = self.pos + 1 + k;
        self.user.clear();
        self.pos = j;
        self.base = j;
        self.current().cloned()
    }

    /// Jumps to the `k`-th song of the next lap shown under "Repeat": the lap becomes the
    /// current one (what was queued by the user is dropped, as with any jump), the one after it is planned.
    fn jump_into_next_lap(&mut self, k: usize) -> Option<Song> {
        if k >= self.next_lap_songs().len() {
            return None;
        }
        self.user.clear();
        self.user_playing = None;
        if let Some(l) = self.next_lap.take() {
            self.context = l;
        }
        self.pos = k;
        self.base = k;
        self.lap_gen += 1;
        self.plan_lap(Repeat::All);
        self.current().cloned()
    }

    /// Previous at the start of MusicKit's queue: restart one song earlier. Returns that song.
    pub fn step_back(&mut self) -> Option<Song> {
        if self.pos == 0 {
            return None;
        }
        self.user_playing = None;
        self.pos -= 1;
        self.base = self.pos;
        self.current().cloned()
    }

    pub fn remove_up_next(&mut self, i: usize) {
        if i < self.user.len() {
            self.user.remove(i);
        } else {
            let j = self.pos + 1 + (i - self.user.len());
            if j < self.context.len() {
                self.context.remove(j);
                self.drop_lap();
            }
        }
    }

    /// Removes several `up_next()` entries (0-based, any order, repeats ignored), highest first
    /// so the numbers stay valid.
    pub fn remove_up_next_many(&mut self, indices: &[usize]) {
        let mut sorted = indices.to_vec();
        sorted.sort_unstable_by(|a, b| b.cmp(a));
        sorted.dedup();
        sorted.into_iter().for_each(|i| self.remove_up_next(i));
    }

    /// Moves entry `from` of `up_next()` so it sits at index `to` afterwards. It joins the user
    /// part when `to` falls inside it (after taking the song out), or at its end when the song
    /// came from there; otherwise it goes into the context. False when either is out of range.
    pub fn move_up_next(&mut self, from: usize, to: usize) -> bool {
        let n = self.user.len() + self.context.len().saturating_sub(self.pos + 1);
        if from >= n || to >= n {
            return false;
        }
        let from_user = from < self.user.len();
        let song = if from_user { self.user.remove(from) } else { Some(self.context.remove(self.pos + 1 + (from - self.user.len()))) };
        let Some(song) = song else { return false };
        if to < self.user.len() || (to == self.user.len() && from_user) {
            self.user.insert(to, song);
            if !from_user {
                self.drop_lap(); // a context song left the lap
            }
        } else {
            let at = (self.pos + 1 + (to - self.user.len())).min(self.context.len());
            self.context.insert(at, song);
            self.drop_lap(); // the lap's end changed (or a song joined it)
        }
        true
    }
}

/// Fisher–Yates with a small xorshift (no crate needed for this), seeded by `seed`.
fn shuffle_in_place<T>(v: &mut [T]) {
    let mut x = seed();
    for i in (1..v.len()).rev() {
        v.swap(i, (xorshift(&mut x) % (i as u64 + 1)) as usize);
    }
}

/// A random playable song of `songs` (where a Shuffle button starts), or None if none is.
pub fn random_playable(songs: &[Song]) -> Option<usize> {
    playable_at(songs, random_below)
}

/// The `pick(n)`-th of the `n` playable songs of `songs` (`random_playable` with its die given).
fn playable_at(songs: &[Song], pick: impl FnOnce(usize) -> usize) -> Option<usize> {
    let playable: Vec<usize> = (0..songs.len()).filter(|&i| songs[i].playable).collect();
    (!playable.is_empty()).then(|| playable[pick(playable.len()).min(playable.len() - 1)])
}

/// A random number below `n` (> 0).
pub fn random_below(n: usize) -> usize {
    (xorshift(&mut seed()) % n.max(1) as u64) as usize
}

/// A fresh seed per call: the clock mixed with a counter, so two shuffles in the same clock tick
/// (macOS reports whole microseconds) still differ. A test can fix the sequence for its thread
/// with `seed_for_tests`.
fn seed() -> u64 {
    #[cfg(test)]
    if let Some(mut x) = TEST_SEED.with(|t| t.get()) {
        let s = splitmix(&mut x);
        TEST_SEED.with(|t| t.set(Some(x)));
        return s | 1;
    }
    static CALLS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let clock = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(0);
    let mut x = clock ^ CALLS.fetch_add(1, std::sync::atomic::Ordering::Relaxed).wrapping_mul(0x9e37_79b9_7f4a_7c15);
    splitmix(&mut x) | 1
}

/// SplitMix64: spreads close inputs (consecutive clock readings) over the whole range.
fn splitmix(x: &mut u64) -> u64 {
    *x = x.wrapping_add(0x9e37_79b9_7f4a_7c15);
    let mut z = *x;
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

#[cfg(test)]
thread_local! {
    static TEST_SEED: std::cell::Cell<Option<u64>> = const { std::cell::Cell::new(None) };
}

/// Tests: every shuffle and random pick on this thread follows from `seed` (each test runs on
/// its own thread), so a failure replays.
#[cfg(test)]
pub fn seed_for_tests(seed: u64) {
    TEST_SEED.with(|t| t.set(Some(seed)));
}

fn xorshift(x: &mut u64) -> u64 {
    *x ^= *x << 13;
    *x ^= *x >> 7;
    *x ^= *x << 17;
    *x
}

#[cfg(test)]
mod tests {
    use super::*;

    fn song(id: &str) -> Song {
        Song {
            id: id.into(),
            catalog_id: None,
            title: id.into(),
            artist: String::new(),
            album: String::new(),
            album_is_fallback: false,
            duration: 100.0,
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

    fn list(n: usize) -> Vec<Song> {
        (0..n).map(|i| song(&i.to_string())).collect()
    }

    fn ids<'a>(it: impl Iterator<Item = &'a Song>) -> Vec<String> {
        it.map(|s| s.id.clone()).collect()
    }

    #[test]
    fn starts_mid_list_and_keeps_earlier_songs_for_previous() {
        let mut q = PlayQueue::start(&list(5), 2, false, "L", VecDeque::new());
        assert_eq!(q.current().unwrap().id, "2");
        assert_eq!(ids(q.up_next()), ["3", "4"]);
        assert_eq!(q.step_back().unwrap().id, "1");
        assert_eq!(ids(q.up_next()), ["2", "3", "4"]);
    }

    #[test]
    fn shuffle_keeps_the_clicked_song_first_and_every_song_once() {
        let q = PlayQueue::start(&list(50), 7, true, "L", VecDeque::new());
        assert_eq!(q.current().unwrap().id, "7");
        let mut all: Vec<_> = q.context.iter().map(|s| s.id.parse::<usize>().unwrap()).collect();
        all.sort();
        assert_eq!(all, (0..50).collect::<Vec<_>>());
    }

    #[test]
    fn a_shuffle_button_can_start_at_any_playable_song() {
        let mut list: Vec<Song> = (0..5).map(|i| song(&i.to_string())).collect();
        list[0].playable = false;
        list[3].playable = false;
        let picks: Vec<_> = (0..4).map(|k| playable_at(&list, |n| { assert_eq!(n, 3); k })).collect();
        assert_eq!(picks, [Some(1), Some(2), Some(4), Some(4)], "only playable songs, an over-roll clamped");
        list.iter_mut().for_each(|s| s.playable = false);
        assert_eq!(playable_at(&list, |_| 0), None);
        assert_eq!(random_playable(&[]), None);
    }

    #[test]
    fn turning_shuffle_off_continues_in_list_order_after_the_current_song() {
        let mut q = PlayQueue::start(&list(10), 3, true, "L", VecDeque::new());
        q.set_shuffle(false);
        assert_eq!(q.current().unwrap().id, "3");
        assert_eq!(ids(q.up_next()), ["4", "5", "6", "7", "8", "9"]);
    }

    #[test]
    fn user_songs_play_first_and_are_consumed_as_they_play() {
        let mut q = PlayQueue::start(&list(4), 0, false, "L", VecDeque::new());
        q.add_to_queue(&[song("b")]);
        q.play_next(&[song("a")]);
        assert_eq!(ids(q.up_next()), ["a", "b", "1", "2", "3"]);
        assert!(q.advance_to(&song("a")));
        assert_eq!(ids(q.up_next()), ["b", "1", "2", "3"]);
        assert!(q.advance_to(&song("b")));
        assert!(q.advance_to(&song("1")));
        assert_eq!(q.pos, 1);
        assert_eq!(ids(q.up_next()), ["2", "3"]);
    }

    #[test]
    fn repeat_all_wraps_to_the_start() {
        let mut q = PlayQueue::start(&list(3), 2, false, "L", VecDeque::new());
        assert!(q.advance_to(&song("0")));
        assert_eq!(q.pos, 0);
        assert_eq!(q.base, 0);
    }

    /// A list 0..9 playing "2", with "7" (a copy of a list song) queued by Play Next.
    fn seven_queued() -> PlayQueue {
        let mut q = PlayQueue::start(&list(10), 2, false, "L", VecDeque::new());
        q.play_next(&[song("7")]);
        assert_eq!(ids(q.up_next()), ["7", "3", "4", "5", "6", "7", "8", "9"]);
        q
    }

    #[test]
    fn a_queued_copy_of_a_list_song_does_not_move_the_position_when_jumped_to() {
        let mut q = seven_queued();
        assert_eq!(q.jump(0).unwrap().id, "7");
        // MusicKit then reports the song it started.
        assert!(q.advance_to(&song("7")));
        assert_eq!(q.pos, 2);
        assert_eq!(ids(q.up_next()), ["3", "4", "5", "6", "7", "8", "9"]);
        // After it, the list goes on at "3".
        assert_eq!(q.peek_next(Repeat::Off).unwrap().id, "3");
        assert_eq!(q.advance(Repeat::Off).unwrap().id, "3");
        assert!(q.user_playing.is_none(), "landing on a list song clears it");
    }

    #[test]
    fn a_duplicate_report_of_the_queued_song_changes_nothing() {
        let mut q = seven_queued();
        assert!(q.advance_to(&song("7")), "natural advance into the queued copy");
        assert_eq!(q.pos, 2);
        assert!(q.advance_to(&song("7")), "the same song reported again");
        assert_eq!(q.pos, 2);
        assert_eq!(ids(q.up_next()), ["3", "4", "5", "6", "7", "8", "9"]);
        assert!(q.advance_to(&song("3")));
        assert_eq!((q.pos, q.user_playing.is_none()), (3, true));
        assert_eq!(ids(q.up_next()), ["4", "5", "6", "7", "8", "9"]);
        // A later report of "7" is the list's own copy again.
        assert!(q.advance_to(&song("7")));
        assert_eq!(q.pos, 7);
    }

    #[test]
    fn advance_off_the_queued_copy_goes_to_the_next_list_song() {
        let mut q = seven_queued();
        assert_eq!(q.advance(Repeat::Off).unwrap().id, "7");
        assert!(q.advance_to(&song("7")));
        assert_eq!(q.pos, 2);
        assert_eq!(q.advance(Repeat::Off).unwrap().id, "3");
        assert_eq!(q.pos, 3);
        assert_eq!(ids(q.up_next()), ["4", "5", "6", "7", "8", "9"]);
    }

    #[test]
    fn a_queued_copy_keeps_the_position_with_shuffle_and_across_a_repeat_all_lap() {
        let mut q = PlayQueue::start(&list(6), 0, true, "L", VecDeque::new());
        q.plan_lap(Repeat::All);
        let last = q.context[5].id.clone();
        q.play_next(&[song(&last)]);
        assert!(q.advance_to(&song(&last)));
        assert_eq!(q.pos, 0);
        assert!(q.advance_to(&song(&last)));
        assert_eq!(q.pos, 0, "not moved to the list's own copy at the end");
        assert_eq!(q.up_next_len(), 5);
        // The lap logic is untouched: the planned lap still comes after the last song.
        let planned: Vec<String> = q.next_lap_songs().iter().map(|s| s.id.clone()).collect();
        let mut sorted = planned.clone();
        sorted.sort();
        assert_eq!(sorted, ["0", "1", "2", "3", "4", "5"]);
        assert!(q.advance_to(&song(&q.context[1].id.clone())));
        assert_eq!(q.pos, 1);
        assert!(q.user_playing.is_none());
    }

    #[test]
    fn step_back_and_a_new_queue_forget_the_queued_song_playing() {
        let mut q = seven_queued();
        q.advance(Repeat::Off);
        assert!(q.user_playing.is_some());
        q.step_back();
        assert!(q.user_playing.is_none());
        let fresh = PlayQueue::start(&list(3), 0, false, "L", VecDeque::new());
        assert!(fresh.user_playing.is_none());
    }

    #[test]
    fn jumping_into_up_next() {
        let mut q = PlayQueue::start(&list(6), 0, false, "L", VecDeque::new());
        q.add_to_queue(&[song("x"), song("y")]);
        assert_eq!(q.jump(1).unwrap().id, "y");
        assert_eq!(ids(q.up_next()), ["1", "2", "3", "4", "5"]);
        assert_eq!(q.jump(2).unwrap().id, "3");
        assert_eq!(ids(q.up_next()), ["4", "5"]);
    }

    fn local(name: &str) -> Song {
        let mut s = song(name);
        s.id = format!("file:///music/{name}.flac");
        s
    }

    #[test]
    fn the_bridge_gets_apple_songs_only_up_to_the_next_local_file() {
        let mixed = vec![song("a"), song("b"), local("x"), song("c")];
        let q = PlayQueue::start(&mixed, 0, false, "L", VecDeque::new());
        assert_eq!(q.upcoming_ids(), ["b"]);
        assert_eq!(q.cycle_ids(), None, "the app wraps mixed lists itself");
        let q = PlayQueue::start(&list(3), 0, false, "L", VecDeque::new());
        assert_eq!(q.cycle_ids().unwrap(), ["0", "1", "2"]);
    }

    #[test]
    fn advance_takes_user_songs_first_then_the_context_and_wraps_only_with_repeat_all() {
        let mut q = PlayQueue::start(&[local("x"), local("y")], 0, false, "L", VecDeque::new());
        q.add_to_queue(&[song("u")]);
        assert_eq!(q.peek_next(Repeat::Off).unwrap().id, "u");
        assert_eq!(q.advance(Repeat::Off).unwrap().id, "u");
        assert_eq!(q.advance(Repeat::Off).unwrap().id, "file:///music/y.flac");
        assert_eq!((q.pos, q.base), (1, 1));
        assert!(q.peek_next(Repeat::Off).is_none());
        assert!(q.advance(Repeat::Off).is_none());
        assert_eq!(q.pos, 1, "nothing moves at the end");
        assert_eq!(q.advance(Repeat::All).unwrap().id, "file:///music/x.flac");
        assert_eq!(q.pos, 0);
    }

    #[test]
    fn repeat_cycles_off_all_one() {
        assert_eq!(Repeat::Off.next(), Repeat::All);
        assert_eq!(Repeat::All.next(), Repeat::One);
        assert_eq!(Repeat::One.next(), Repeat::Off);
    }

    #[test]
    fn move_up_next_lands_at_the_target_index() {
        let q0 = {
            let mut q = PlayQueue::start(&list(6), 0, false, "L", VecDeque::new());
            q.add_to_queue(&[song("a"), song("b")]);
            q
        };
        assert_eq!(ids(q0.up_next()), ["a", "b", "1", "2", "3", "4", "5"]);
        let moved = |from, to| {
            let mut q = q0.clone();
            assert!(q.move_up_next(from, to));
            (ids(q.up_next()).join(" "), q.user.len())
        };
        // Within the user part, and to its end.
        assert_eq!(moved(0, 1), ("b a 1 2 3 4 5".to_string(), 2));
        assert_eq!(moved(1, 0), ("b a 1 2 3 4 5".to_string(), 2));
        // Context song into the user part.
        assert_eq!(moved(4, 0), ("3 a b 1 2 4 5".to_string(), 3));
        assert_eq!(moved(4, 1), ("a 3 b 1 2 4 5".to_string(), 3));
        // To == user.len() from the context stays in the context (first song of the rest).
        assert_eq!(moved(4, 2), ("a b 3 1 2 4 5".to_string(), 2));
        // User song out into the context.
        assert_eq!(moved(0, 4), ("b 1 2 3 a 4 5".to_string(), 1));
        assert_eq!(moved(1, 6), ("a 1 2 3 4 5 b".to_string(), 1));
        // Within the context, both ways.
        assert_eq!(moved(2, 5), ("a b 2 3 4 1 5".to_string(), 2));
        assert_eq!(moved(6, 2), ("a b 5 1 2 3 4".to_string(), 2));
        // Same place changes nothing.
        assert_eq!(moved(3, 3), ("a b 1 2 3 4 5".to_string(), 2));
    }

    #[test]
    fn move_up_next_rejects_out_of_range_and_keeps_the_current_song() {
        let mut q = PlayQueue::start(&list(4), 1, false, "L", VecDeque::new());
        assert_eq!(ids(q.up_next()), ["2", "3"]);
        assert!(!q.move_up_next(2, 0));
        assert!(!q.move_up_next(0, 2));
        assert!(q.move_up_next(0, 1));
        assert_eq!(ids(q.up_next()), ["3", "2"]);
        assert_eq!(q.current().unwrap().id, "1");
        let mut empty = PlayQueue::default();
        assert!(!empty.move_up_next(0, 0));
    }

    #[test]
    fn remove_many_ignores_order_and_repeats() {
        let mut q = PlayQueue::start(&list(6), 0, false, "L", VecDeque::new());
        q.add_to_queue(&[song("a"), song("b")]);
        q.remove_up_next_many(&[3, 0, 3, 5]);
        assert_eq!(ids(q.up_next()), ["b", "1", "3", "5"]);
        assert_eq!(q.user.len(), 1);
        q.remove_up_next_many(&[]);
        assert_eq!(ids(q.up_next()).len(), 4);
    }

    fn order(q: &PlayQueue) -> Vec<String> {
        q.context.iter().map(|s| s.id.clone()).collect()
    }

    fn shown(q: &PlayQueue) -> Vec<String> {
        q.next_lap_songs().iter().map(|s| s.id.clone()).collect()
    }

    fn sorted(mut v: Vec<String>) -> Vec<String> {
        v.sort_by_key(|s| s.parse::<usize>().unwrap_or(usize::MAX));
        v
    }

    #[test]
    fn shuffled_repeat_all_reshuffles_each_lap_and_plays_every_song_once() {
        let mut q = PlayQueue::start(&list(40), 5, true, "L", VecDeque::new());
        assert!(q.plan_lap(Repeat::All), "planned as soon as the lap is known, not on its last song");
        let mut prev = order(&q);
        for _ in 0..3 {
            let planned = shown(&q);
            assert_ne!(planned, prev, "a fresh order");
            q.pos = q.context.len() - 1;
            let last = q.context[q.pos].clone();
            assert_ne!(planned[0], last.id, "never opens with the song that ended the lap");
            assert_eq!(shown(&q), planned, "being on the last song changes nothing");
            assert_eq!(q.cycle_ids().unwrap(), planned, "the bridge loops into what is shown");
            assert_eq!(q.peek_next(Repeat::All).unwrap().id, planned[0]);
            let first = q.advance(Repeat::All).unwrap();
            assert_eq!((first.id.clone(), q.pos, q.base), (planned[0].clone(), 0, 0));
            assert_eq!(order(&q), planned, "what was shown is what plays");
            assert_eq!(sorted(order(&q)), (0..40).map(|i| i.to_string()).collect::<Vec<_>>());
            assert_eq!(shown(&q).len(), 40, "and the lap after it is already shuffled");
            assert_ne!(shown(&q), order(&q));
            prev = order(&q);
        }
    }

    #[test]
    fn what_up_next_shows_is_exactly_what_plays_over_two_laps() {
        let mut q = PlayQueue::start(&list(8), 3, true, "L", VecDeque::new());
        q.plan_lap(Repeat::All);
        // Up Next, then the divider, then the lap: the whole sequence the next 12 songs follow.
        let mut want: Vec<String> = ids(q.up_next());
        want.extend(shown(&q));
        assert_eq!(want.len(), 7 + 8);
        let mut played = Vec::new();
        for _ in 0..want.len() {
            played.push(q.advance(Repeat::All).unwrap().id);
        }
        assert_eq!(played, want);
    }

    #[test]
    fn musickit_looping_on_its_own_lands_in_the_shown_lap_and_a_new_one_is_planned() {
        let mut q = PlayQueue::start(&list(30), 0, true, "L", VecDeque::new());
        assert!(q.plan_lap(Repeat::All));
        assert!(!q.plan_lap(Repeat::All), "planned once");
        let (planned, g0) = (shown(&q), q.lap_gen);
        // The first song of the shown lap is not taken for the wrap before the last song.
        q.pos = 10;
        assert!(q.advance_to(&song(&planned[0])));
        assert_eq!(q.lap_gen, g0, "it was an ordinary jump inside this lap");
        assert_eq!(order(&q).len(), 30);
        let mut q = PlayQueue::start(&list(30), 0, true, "L", VecDeque::new());
        q.plan_lap(Repeat::All);
        let planned = shown(&q);
        q.pos = 29;
        assert!(q.advance_to(&song(&planned[0])));
        assert_eq!((q.pos, q.base), (0, 0));
        assert_eq!(order(&q), planned);
        assert!(q.lap_gen > g0);
        assert_eq!(shown(&q).len(), 30, "the next lap is planned at the hand-over");
        assert_ne!(shown(&q), planned);
    }

    #[test]
    fn two_songs_swap_every_lap_and_one_song_repeats() {
        let mut q = PlayQueue::start(&list(2), 0, true, "L", VecDeque::new());
        q.pos = 1;
        let last = q.context[1].id.clone();
        assert_ne!(q.advance(Repeat::All).unwrap().id, last);
        let mut one = PlayQueue::start(&list(1), 0, true, "L", VecDeque::new());
        assert_eq!(one.advance(Repeat::All).unwrap().id, "0");
    }

    #[test]
    fn toggling_shuffle_or_repeat_recomputes_the_next_lap() {
        let mut q = PlayQueue::start(&list(10), 0, true, "L", VecDeque::new());
        q.plan_lap(Repeat::All);
        assert_eq!(shown(&q).len(), 10);
        assert!(q.plan_lap(Repeat::Off), "repeat off drops it");
        assert!(q.next_lap_songs().is_empty());
        assert_eq!(q.cycle_ids().unwrap().len(), 10);
        q.plan_lap(Repeat::One);
        assert!(q.next_lap_songs().is_empty(), "repeat one shows no lap");
        q.plan_lap(Repeat::All);
        assert_eq!(shown(&q).len(), 10);
        q.set_shuffle(false);
        assert!(q.next_lap.is_none());
        assert!(q.next_lap_songs().is_empty() || q.shuffled == false, "stale plan gone until replanned");
        assert!(!q.plan_lap(Repeat::All), "nothing stored without shuffle");
        assert_eq!(shown(&q), order(&q), "without shuffle the next lap is the list order");
        assert_eq!(q.cycle_ids().unwrap(), order(&q));
        q.set_shuffle(true);
        q.plan_lap(Repeat::All);
        assert_eq!(sorted(shown(&q)), (0..10).map(|i| i.to_string()).collect::<Vec<_>>());
    }

    #[test]
    fn edits_of_the_lap_recompute_it_and_edits_of_the_user_part_leave_it() {
        let mut q = PlayQueue::start(&list(10), 0, true, "L", VecDeque::new());
        q.plan_lap(Repeat::All);
        let planned = shown(&q);
        // Play Next / Add to Queue and moves among user songs don't touch the lap.
        q.add_to_queue(&[song("a"), song("b")]);
        q.play_next(&[song("c")]);
        assert!(q.move_up_next(0, 2));
        assert!(!q.plan_lap(Repeat::All));
        assert_eq!(shown(&q), planned);
        // Removing a user song neither.
        q.remove_up_next(0);
        assert!(!q.plan_lap(Repeat::All));
        assert_eq!(shown(&q), planned);
        // Removing a context song drops the lap; the new one has one song fewer, none of the removed.
        let gone = q.context[q.pos + 1].id.clone();
        q.remove_up_next(q.user.len());
        assert!(q.next_lap_songs().is_empty(), "stale until replanned");
        assert!(q.plan_lap(Repeat::All));
        assert_eq!(shown(&q).len(), 9);
        assert!(!shown(&q).contains(&gone));
        // A context song moved around, or out to the user part, or a user song into the context.
        let check = |q: &mut PlayQueue, f: &dyn Fn(&mut PlayQueue)| {
            f(q);
            q.plan_lap(Repeat::All);
            assert_eq!(sorted(shown(q)), sorted(order(q)), "the lap has exactly the context's songs");
            assert_eq!(shown(q).len(), q.context.len());
        };
        check(&mut q, &|q| assert!(q.move_up_next(q.user.len() + 1, q.user.len() + 3)));
        check(&mut q, &|q| assert!(q.move_up_next(q.user.len() + 2, 0)));
        let last = q.up_next_len() - 1;
        check(&mut q, &|q| assert!(q.move_up_next(0, last)));
    }

    #[test]
    fn the_lap_never_opens_with_the_last_two_songs_of_this_one() {
        for n in [3usize, 4, 5, 8] {
            for _ in 0..200 {
                let mut q = PlayQueue::start(&list(n), 0, true, "L", VecDeque::new());
                q.plan_lap(Repeat::All);
                let lap = shown(&q);
                let c = order(&q);
                assert!(lap[0] != c[n - 1] && lap[0] != c[n - 2], "{c:?} -> {lap:?}");
            }
        }
    }

    #[test]
    fn jumping_into_the_next_lap_makes_it_the_current_one() {
        let mut q = PlayQueue::start(&list(6), 3, true, "L", VecDeque::new());
        q.plan_lap(Repeat::All);
        q.add_to_queue(&[song("u")]);
        let lap = shown(&q);
        let n = q.up_next_len();
        assert_eq!(n, 6, "the user song and the 5 others");
        assert_eq!(q.jump(n + 2).unwrap().id, lap[2]);
        assert_eq!(order(&q), lap, "the shown lap is the context now");
        assert_eq!((q.pos, q.base), (2, 2));
        assert!(q.user.is_empty());
        assert_eq!(shown(&q).len(), 6, "a new next lap follows");
        assert_eq!(q.jump(q.up_next_len() + 6), None, "past the lap");
        // Without shuffle: the same order, from that song.
        let mut q = PlayQueue::start(&list(4), 2, false, "L", VecDeque::new());
        q.plan_lap(Repeat::All);
        assert_eq!(ids(q.up_next()), ["3"]);
        assert_eq!(shown(&q), ["0", "1", "2", "3"]);
        assert_eq!(q.jump(2).unwrap().id, "1");
        assert_eq!(ids(q.up_next()), ["2", "3"]);
        // Repeat off: no lap to jump into.
        q.plan_lap(Repeat::Off);
        assert_eq!(q.jump(2 + 4), None);
    }

    #[test]
    fn repeat_all_without_shuffle_keeps_list_order_on_every_path() {
        let mut q = PlayQueue::start(&list(4), 3, false, "L", VecDeque::new());
        assert!(!q.plan_lap(Repeat::All));
        assert_eq!(q.advance(Repeat::All).unwrap().id, "0");
        assert_eq!(order(&q), ["0", "1", "2", "3"]);
    }

    fn on_station(heard: &[&str]) -> PlayQueue {
        let mut q = PlayQueue::for_station("ra.1", "Stazione", None);
        for h in heard {
            q.station_heard(&song(h));
        }
        q
    }

    fn played(q: &PlayQueue) -> Vec<String> {
        q.station_played().iter().map(|s| s.id.clone()).collect()
    }

    #[test]
    fn a_station_remembers_what_it_played_newest_first_without_the_current_song() {
        let mut q = on_station(&["a"]);
        assert!(played(&q).is_empty(), "the first song is the current one, not history");
        q.station_heard(&song("a"));
        assert!(played(&q).is_empty(), "the same song reported again changes nothing");
        q.station_heard(&song("b"));
        q.station_heard(&song("c"));
        assert_eq!(played(&q), ["b", "a"]);
        q.station_heard(&song("a"));
        assert_eq!(played(&q), ["c", "b"], "a song heard again leaves the history while it plays");
        assert_eq!(q.up_next_len(), 0);
        // No station: nothing is recorded.
        let mut plain = PlayQueue::start(&list(3), 0, false, "L", VecDeque::new());
        plain.station_heard(&song("x"));
        assert!(plain.station_played().is_empty());
    }

    #[test]
    fn the_history_is_capped() {
        let mut q = PlayQueue::for_station("ra.1", "S", None);
        for i in 0..STATION_HISTORY + 20 {
            q.station_heard(&song(&format!("s{i}")));
        }
        let p = played(&q);
        assert_eq!(p.len(), STATION_HISTORY);
        assert_eq!(p[0], format!("s{}", STATION_HISTORY + 18), "newest first");
    }

    #[test]
    fn clicking_a_played_song_plays_it_alone_and_keeps_what_was_queued_by_hand() {
        let mut q = on_station(&["a", "b", "c", "d"]);
        q.add_to_queue(&[song("u")]);
        assert_eq!(played(&q), ["c", "b", "a"]);
        // Entries: user song 0, then history from 1.
        let s = q.jump(2).unwrap();
        assert_eq!(s.id, "b");
        assert_eq!(q.current().unwrap().id, "b");
        assert_eq!(played(&q), ["d", "c", "a"], "the song that was playing joins the history");
        assert_eq!(ids(q.up_next()), ["u"], "the queued song still comes next");
        assert!(q.station.as_ref().unwrap().one_off);
        // After the one-off the queue has nothing: no wrap, even with repeat all.
        assert!(q.advance_to(&song("u")));
        assert!(q.peek_next(Repeat::All).is_none());
        assert!(q.cycle_ids().is_none());
        assert!(q.advance(Repeat::All).is_none());
        assert!(!q.plan_lap(Repeat::All));
        assert!(q.next_lap_songs().is_empty());
    }

    #[test]
    fn the_station_picks_up_with_its_history_and_numbers_past_the_end_do_nothing() {
        let mut q = on_station(&["a", "b"]);
        assert_eq!(q.jump(0).unwrap().id, "a");
        assert_eq!(q.jump(5), None);
        let run = q.station.clone();
        let q2 = PlayQueue::for_station("ra.1", "Stazione", run);
        let st = q2.station.as_ref().unwrap();
        assert!(!st.one_off);
        assert_eq!(st.heard.as_ref().unwrap().id, "a");
        assert_eq!(played(&q2), ["b"]);
        assert_eq!(q2.source, "Stazione");
    }

    #[test]
    fn an_unavailable_song_in_the_history_is_not_replayed() {
        let mut q = on_station(&["a", "b"]);
        q.station.as_mut().unwrap().played[0].playable = false;
        assert_eq!(q.jump(0), None);
        assert!(!q.station.as_ref().unwrap().one_off);
    }
    #[test]
    fn songs_queued_on_a_live_station_are_the_apps_and_play_as_one_offs() {
        let mut q = on_station(&["a"]);
        assert!(q.station_live());
        q.add_to_queue(&[song("u1"), song("u2")]);
        assert!(q.upcoming_ids().is_empty(), "MusicKit never gets them behind a station");
        assert_eq!(q.cycle_ids(), None);
        assert!(!q.bridge_has_next(Repeat::Off), "the app must take over");
        assert!(!q.bridge_has_next(Repeat::All));
        let s = q.station_take_user().unwrap();
        assert_eq!(s.id, "u1");
        assert!(!q.station_live() && q.station.as_ref().unwrap().one_off);
        assert_eq!(ids(q.context.iter()), ["u1"]);
        assert_eq!(q.current().unwrap().id, "u1");
        assert_eq!(played(&q), ["a"], "the station's song did play: it joins the history");
        assert_eq!(q.upcoming_ids(), ["u2"], "the rest follows in MusicKit's queue, as a list");
        assert!(q.bridge_has_next(Repeat::Off));
        assert_eq!(q.station_take_user(), None, "only while the station itself picks");
        assert!(q.advance_to(&song("u2")));
        assert!(q.user.is_empty() && q.peek_next(Repeat::All).is_none(), "then the station resumes");
    }

    #[test]
    fn advance_and_jump_on_a_live_station_take_the_queued_song_as_a_one_off() {
        let mut q = on_station(&["a"]);
        assert_eq!(q.advance(Repeat::All), None, "nothing queued: the station goes on by itself");
        assert!(q.station_live());
        let mut gone = song("x");
        gone.playable = false;
        q.add_to_queue(&[gone, song("u")]);
        assert_eq!(q.advance(Repeat::Off).unwrap().id, "u", "an unavailable song is dropped");
        assert!(q.station.as_ref().unwrap().one_off);
        let mut q = on_station(&["a"]);
        q.add_to_queue(&[song("u1"), song("u2"), song("u3")]);
        assert_eq!(q.jump(1).unwrap().id, "u2");
        assert!(q.station.as_ref().unwrap().one_off);
        assert_eq!(ids(q.up_next()), ["u3"]);
        assert_eq!(q.upcoming_ids(), ["u3"]);
    }

    #[test]
    fn a_new_station_pick_with_songs_queued_means_the_hand_over_was_missed() {
        let mut q = PlayQueue::for_station("ra.1", "S", None);
        q.add_to_queue(&[song("u")]);
        assert!(!q.station_skipped_queue(&song("a")), "the station's first song plays");
        q.station_heard(&song("a"));
        assert!(!q.station_skipped_queue(&song("a")), "the same song reported again");
        assert!(!q.station_skipped_queue(&song("u")), "the queued song itself");
        assert!(q.station_skipped_queue(&song("b")));
        q.user.clear();
        assert!(!q.station_skipped_queue(&song("b")), "nothing queued: the station's song is fine");
        q.add_to_queue(&[song("u")]);
        q.station_take_user();
        assert!(!q.station_skipped_queue(&song("b")), "not during a one-off");
        // No station: never.
        let mut plain = PlayQueue::start(&list(2), 0, false, "L", VecDeque::new());
        plain.add_to_queue(&[song("u")]);
        assert!(!plain.station_skipped_queue(&song("z")));
        assert_eq!(plain.station_take_user(), None);
        assert_eq!(plain.upcoming_ids(), ["u", "1"]);
    }
}
