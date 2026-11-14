//! The control socket for `uzu`, the mpc-style command line (`ctl/`, protocol in `uzumaki_ctl`).
//!
//! Each request runs on its connection's thread. State reads and actions hop to the UI thread
//! (`AppHandle::update_if`) and wait for the answer there. Searches and album/playlist fetches
//! call the bridge from here, so a slow Apple reply never holds up the UI.
//!
//! The bridge is asked of `State` for each call, so one started after launch (the onboarding's
//! choice, Settings → Account) is seen, and none in local only mode. Commands that need Apple
//! Music then answer so (`needs_apple`).

use std::collections::HashMap;
use std::sync::{Mutex, mpsc};
use std::time::{Duration, SystemTime};

use serde_json::Value;
use uzumaki_ctl::{CollectionInfo, Kind, PickAction, Reply, Request, Server};

use crate::cache::Cache;
use crate::models::{Collection, Page, SearchResults, Song, api};
use crate::state::ctl::{cached_is_current, collection_info, is_current, repeat_from, song_info};
use crate::state::{Handle, State, TRACKS_CAP};

/// How long a request waits for the UI thread (it answers within a frame unless it's stuck).
const UI_TIMEOUT: Duration = Duration::from_secs(5);
/// How long `Quit` waits after answering, so the client reads the reply first.
const QUIT_DELAY: Duration = Duration::from_millis(200);
/// How long `uzu airplay` looks for speakers before answering.
const AIRPLAY_LOOK: Duration = Duration::from_secs(3);
/// How long one Apple Music API call may take.
const API_TIMEOUT: Duration = Duration::from_secs(20);

/// Starts listening. The socket file goes away when the returned server is dropped.
pub fn start(handle: Handle) -> Option<Server> {
    let ctl = Ctl { handle, cache: Cache::default_location(), last_search: Mutex::default(), disk_counts: Mutex::default() };
    match uzumaki_ctl::serve(&uzumaki_ctl::socket_path(), move |req| ctl.answer(req)) {
        Ok(server) => Some(server),
        Err(e) => {
            eprintln!("uzumaki: no control socket, so uzu won't work: {e}");
            None
        }
    }
}

/// Whether another copy answers on the control socket; `show` asks it to show its window. On
/// macOS there's no session bus, so this stands in for the MPRIS single-instance check: without
/// it a second launch would start a second bridge on the same login profile.
pub fn other_copy_running(show: bool) -> bool {
    let Ok(mut client) = uzumaki_ctl::Client::connect() else { return false };
    if show {
        let _ = client.request(&Request::Show);
    }
    true
}

struct Ctl {
    handle: Handle,
    cache: Cache,
    /// The last `uzu search`: its words and songs, for `uzu pick/insert/add`.
    last_search: Mutex<(String, Vec<Song>)>,
    disk_counts: Mutex<DiskCounts>,
}

/// Playlist song counts read from the disk cache: id → (the listed `modified`, the file's time,
/// the count). Read again when either changes.
type DiskCounts = HashMap<String, (Option<String>, SystemTime, Option<u32>)>;

/// Fills the counts Apple's playlist list doesn't give (library playlists have no trackCount)
/// and the app doesn't hold in memory, from the songs cached on disk (the first sync saves every
/// playlist's), when that copy is current. No request to Apple; each file is read once.
fn fill_counts(cache: &Cache, memo: &mut DiskCounts, entries: Vec<(CollectionInfo, Option<String>)>) -> Vec<CollectionInfo> {
    entries
        .into_iter()
        .map(|(mut info, listed)| {
            if info.count.is_none() {
                info.count = disk_count(cache, memo, &info.id, listed.as_deref());
            }
            info
        })
        .collect()
}

fn disk_count(cache: &Cache, memo: &mut DiskCounts, id: &str, listed: Option<&str>) -> Option<u32> {
    let time = std::fs::metadata(cache.collection_path(id)).and_then(|m| m.modified()).ok()?;
    if let Some((m, t, n)) = memo.get(id) {
        if m.as_deref() == listed && *t == time {
            return *n;
        }
    }
    let n = cache
        .collection_count(id)
        .filter(|c| c.count > 0 && is_current(id, c.saved_at, c.modified.as_deref(), listed))
        .map(|c| c.count as u32);
    memo.insert(id.to_string(), (listed.map(String::from), time, n));
    n
}

impl Ctl {
    fn answer(&self, req: Request) -> Reply {
        self.run(req).unwrap_or_else(|message| Reply::Error { message })
    }

    fn run(&self, req: Request) -> Result<Reply, String> {
        use Request as R;
        if let Some(what) = needs_apple(&req).filter(|_| !local_answers(&req)) {
            if self.on_ui(false, |s| s.local_only())? {
                return Err(crate::state::local_only_answer(what));
            }
        }
        match req {
            R::Status => self.read(|s| Reply::Status(s.ctl_status())),
            R::Play => self.act(|s| s.play()),
            R::Pause => self.act(|s| s.pause()),
            R::Toggle => self.act(|s| s.toggle_play()),
            R::Next => self.act(|s| s.next()),
            R::Prev => self.act(|s| s.previous()),
            R::Seek { to } => self.act(move |s| s.ctl_seek(to)),
            R::Volume { to } => self.act(move |s| s.ctl_volume(to)),
            R::Shuffle { on } => self.act(move |s| s.set_shuffle(on.unwrap_or(!s.playback.shuffle))),
            R::Repeat { mode } => self.act(move |s| s.set_repeat(mode.map_or(s.playback.repeat.next(), repeat_from))),
            R::Autoplay { on } => self.on_ui(true, move |s| {
                if !s.autoplay_offered() {
                    return Err("AutoPlay isn't offered here (local files only, or a station is playing)".to_string());
                }
                s.set_autoplay(on.unwrap_or(!s.settings.autoplay));
                Ok(Reply::Queue(s.ctl_queue()))
            })?,
            R::Playlists => {
                let entries = self.on_ui(false, |s| s.ctl_playlist_entries())?;
                let mut memo = self.disk_counts.lock().unwrap_or_else(|e| e.into_inner());
                Ok(Reply::Collections { items: fill_counts(&self.cache, &mut memo, entries) })
            }
            R::Albums => self.read(|s| Reply::Collections { items: s.ctl_albums() }),
            R::Tracks { kind, id } => {
                let c = self.collection(kind, &id)?;
                Ok(Reply::Tracks { collection: collection_info(&c), songs: c.tracks.iter().map(song_info).collect() })
            }
            R::PlayCollection { kind, id, index, order } => {
                let c = self.collection(kind, &id)?;
                self.try_act(move |s| s.ctl_play_collection(&c, index, order.as_deref()))
            }
            R::Queue => self.read(|s| Reply::Queue(s.ctl_queue())),
            R::History { index } => self.try_act(move |s| {
                let Some(&i) = s.history_indices().get(index) else { return Err("History entry no longer exists".into()); };
                s.play_history(i); Ok(())
            }),
            R::Jump { index } => self.try_act(move |s| s.ctl_jump(index)),
            R::Move { from, to } => self.on_ui(true, move |s| s.ctl_move(from, to).map(|()| Reply::Queue(s.ctl_queue())))?,
            R::Remove { indices } => self.on_ui(true, move |s| s.ctl_remove(&indices).map(|()| Reply::Queue(s.ctl_queue())))?,
            R::Search { query } => self.search(&query),
            R::Pick { indices, action } => self.pick(&indices, action),
            // From memory: `ctl_lyrics` never calls the bridge except to fetch lyrics that are
            // missing, once (this thread doesn't wait for Apple; the next poll sees the result).
            R::Lyrics => self.on_ui(true, |s| s.ctl_lyrics()),
            R::AirPlay { to: uzumaki_ctl::AirPlayTo::List } => {
                // Speakers are looked for only on request: for a moment here, as while the picker is open.
                self.on_ui(false, |s| {
                    if !s.airplay.available {
                        return Err(crate::state::airplay::AIRPLAY_UNAVAILABLE.to_string());
                    }
                    s.airplay_ctl_browse(true);
                    Ok(())
                })??;
                std::thread::sleep(AIRPLAY_LOOK);
                self.on_ui(false, |s| {
                    s.airplay_ctl_browse(false);
                    s.ctl_airplay()
                })
            }
            R::AirPlay { to: uzumaki_ctl::AirPlayTo::Off } => self.try_act(|s| {
                if !s.airplay.available {
                    return Err(crate::state::airplay::AIRPLAY_UNAVAILABLE.to_string());
                }
                s.airplay_pick(None);
                Ok(())
            }),
            R::AirPlay { to: uzumaki_ctl::AirPlayTo::Name(name) } => self.try_act(move |s| s.airplay_pick_named(&name)),
            R::Quit => {
                let reply = self.read(|s| Reply::Status(s.ctl_status()));
                // Quit a moment after answering, so the reply is written before the socket goes.
                let handle = self.handle.clone();
                std::thread::spawn(move || {
                    std::thread::sleep(QUIT_DELAY);
                    handle.quit();
                });
                reply
            }
            R::Show => {
                crate::tray::show_window(&self.handle);
                self.read(|s| Reply::Status(s.ctl_status()))
            }
        }
    }

    // ------------------------------------------------------------------------ UI thread

    /// Runs `f` on the UI thread and waits for its result. `redraw`: whether the view may change.
    fn on_ui<T: Send + 'static>(&self, redraw: bool, f: impl FnOnce(&mut State) -> T + Send + 'static) -> Result<T, String> {
        let (tx, rx) = mpsc::sync_channel(1);
        self.handle.update_if(move |s, _| {
            let _ = tx.send(f(s));
            redraw
        });
        rx.recv_timeout(UI_TIMEOUT).map_err(|_| "Uzumaki didn't answer in time".to_string())
    }

    fn read(&self, f: impl FnOnce(&State) -> Reply + Send + 'static) -> Result<Reply, String> {
        self.on_ui(false, move |s| f(s))
    }

    /// An action, answered with the status after it.
    fn act(&self, f: impl FnOnce(&mut State) + Send + 'static) -> Result<Reply, String> {
        self.try_act(move |s| {
            f(s);
            Ok(())
        })
    }

    fn try_act(&self, f: impl FnOnce(&mut State) -> Result<(), String> + Send + 'static) -> Result<Reply, String> {
        self.on_ui(true, move |s| f(s).map(|()| Reply::Status(s.ctl_status())))?
    }

    // ------------------------------------------------------------------------ Apple

    fn api(&self, (path, params): (String, Value)) -> Result<Value, String> {
        let b = self.on_ui(false, |s| s.apple_bridge())??;
        let (tx, rx) = mpsc::sync_channel(1);
        b.api(&path, params, move |r| {
            let _ = tx.send(r);
            None
        });
        rx.recv_timeout(API_TIMEOUT).map_err(|_| "Apple Music didn't answer in time".to_string())?
    }

    /// An album or playlist with its songs: from memory, else the disk cache when it's current,
    /// else Apple (a stale disk copy still beats an error).
    fn collection(&self, kind: Kind, id: &str) -> Result<Collection, String> {
        let key = id.to_string();
        let (loaded, listed) = self.on_ui(false, move |s| s.ctl_lookup(&key))?;
        if let Some(c) = loaded {
            return Ok(c);
        }
        if crate::local::view::is_album_id(id) {
            return Err("that album is no longer in the music folders".into());
        }
        let cached = self.cache.read_collection(id).filter(|c| !c.data.tracks.is_empty());
        if let Some(stored) = &cached {
            if cached_is_current(stored, listed.as_ref().and_then(|l| l.modified.as_deref())) {
                self.adopt(stored.data.clone(), Some(stored.saved_at));
                return Ok(stored.data.clone());
            }
        }
        match self.fetch(kind, id) {
            Ok(c) => {
                self.adopt(c.clone(), None);
                Ok(c)
            }
            Err(e) => cached.map(|stored| stored.data).ok_or(e),
        }
    }

    fn adopt(&self, c: Collection, saved_at: Option<u64>) {
        self.handle.update_if(move |s, _| {
            s.ctl_adopt(c, saved_at);
            false
        });
    }

    fn fetch(&self, kind: Kind, id: &str) -> Result<Collection, String> {
        let req = match kind {
            Kind::Album => api::album_with(id, false), // the command line has no use for the loop
            Kind::Playlist => api::playlist(id),
        };
        let v = self.api(req)?;
        let d = &v["data"][0];
        let mut next = d["relationships"]["tracks"]["next"].as_str().map(String::from);
        let mut c = Collection::parse(d).ok_or("Apple Music doesn't know that album or playlist")?;
        while let Some(path) = next.take() {
            if c.tracks.len() >= TRACKS_CAP {
                break;
            }
            let page = Page::parse(&self.api(api::next(&path, 100))?, Song::parse);
            c.tracks.extend(page.items);
            next = page.next;
        }
        Ok(c)
    }

    fn search(&self, query: &str) -> Result<Reply, String> {
        let query = query.trim();
        if query.is_empty() {
            return Err("search for what?".into());
        }
        let songs = SearchResults::parse(&self.api(api::search(query))?).songs;
        let reply = Reply::Songs { songs: songs.iter().map(song_info).collect() };
        *self.last_search.lock().unwrap_or_else(|e| e.into_inner()) = (query.to_string(), songs);
        Ok(reply)
    }

    fn pick(&self, indices: &[usize], action: PickAction) -> Result<Reply, String> {
        let (source, songs) = {
            let last = self.last_search.lock().unwrap_or_else(|e| e.into_inner());
            if last.1.is_empty() {
                return Err("search first: uzu search <words>".into());
            }
            let songs = indices
                .iter()
                .map(|&i| last.1.get(i).cloned().ok_or_else(|| format!("the last search has no result {}", i + 1)))
                .collect::<Result<Vec<Song>, String>>()?;
            (format!("Search “{}”", last.0), songs)
        };
        if songs.is_empty() {
            return Err("pick which? uzu pick <n>".into());
        }
        self.try_act(move |s| s.ctl_pick(songs, action, &source))
    }
}

/// Commands that need Apple Music, by the name `uzu` gives them (`None`: they work in local
/// only mode too: playback, the queue, status, history).
/// Requests the music folders answer in local only mode: the albums list and a local album's
/// songs (`local/view.rs` ids; Apple ids still need Apple).
fn local_answers(req: &Request) -> bool {
    use crate::local::view::is_album_id;
    match req {
        Request::Albums => true,
        Request::Tracks { id, .. } | Request::PlayCollection { id, .. } => is_album_id(id),
        _ => false,
    }
}

pub fn needs_apple(req: &Request) -> Option<&'static str> {
    use Request as R;
    Some(match req {
        R::Search { .. } => "uzu search",
        R::Pick { .. } => "uzu pick",
        R::Playlists => "uzu playlists",
        R::Albums => "uzu albums",
        R::Tracks { .. } => "uzu tracks",
        R::PlayCollection { .. } => "uzu play of an album or playlist",
        R::Lyrics => "uzu lyrics",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn info(id: &str, count: Option<u32>) -> CollectionInfo {
        CollectionInfo { kind: Kind::Playlist, id: id.into(), title: id.into(), subtitle: String::new(), count, favorites: false, editable: true, year: None, pinned: None }
    }

    #[test]
    fn playlist_counts_come_from_the_disk_cache_when_current() {
        let dir = std::env::temp_dir().join(format!("uzumaki-ctl-counts-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let cache = Cache::at(dir.clone());
        let save = |id: &str, modified: &str, n: usize| {
            let tracks: Vec<Value> = (0..n).map(|i| json!({"id": i.to_string()})).collect();
            cache.write(&Cache::collection_name(id), &json!({"id": id, "modified": modified, "tracks": tracks})).unwrap();
        };
        save("p.wave", "v1", 3);
        save("p.lose", "v1", 2);
        save("p.old", "v1", 4);
        save("p.empty", "v1", 0);
        let entries = || {
            vec![
                (info("p.wave", Some(9)), Some("v1".to_string())), // the app's own count wins
                (info("p.lose", None), Some("v1".to_string())),
                (info("p.old", None), Some("v2".to_string())), // edited since it was cached
                (info("p.empty", None), Some("v1".to_string())),
                (info("p.none", None), Some("v1".to_string())), // nothing on disk
            ]
        };
        let mut memo = DiskCounts::new();
        let counts: Vec<_> = fill_counts(&cache, &mut memo, entries()).iter().map(|c| c.count).collect();
        assert_eq!(counts, [Some(9), Some(2), None, None, None]);
        // Each file is read once: a second listing answers from memory.
        assert_eq!(memo.len(), 3);
        let again: Vec<_> = fill_counts(&cache, &mut memo, entries()).iter().map(|c| c.count).collect();
        assert_eq!(again, counts);
        // The playlist changed and was cached again: read again.
        std::thread::sleep(Duration::from_millis(20));
        save("p.old", "v2", 5);
        let after: Vec<_> = fill_counts(&cache, &mut memo, entries()).iter().map(|c| c.count).collect();
        assert_eq!(after[2], Some(5));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
