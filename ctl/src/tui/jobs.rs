//! Slow requests (a playlist's songs, a search) and encoding the cover run off the UI thread so
//! the screen never freezes. [`Threaded`] runs each on its own thread and connection; tests use
//! [`Inline`].

use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use ratatui_image::protocol::Protocol;
use uzumaki_ctl::{CollectionInfo, Kind, Reply, Request, SongInfo};

use super::Reconnecting;
use super::cover::{self, Key};
use crate::run::Backend;

pub enum Job {
    Tracks { kind: Kind, id: String },
    Search { seq: u64, query: String },
    /// Reads and encodes a cover file (no request to the app).
    Cover { key: Key, kitty_id: u32 },
}

pub enum Done {
    Tracks { kind: Kind, id: String, result: Result<(CollectionInfo, Vec<SongInfo>), String> },
    Search { seq: u64, result: Result<Vec<SongInfo>, String> },
    Cover { key: Key, kitty_id: u32, result: Result<Protocol, String> },
}

pub trait Jobs {
    fn submit(&mut self, job: Job);
    /// Results that finished since the last call.
    fn poll(&mut self) -> Vec<Done>;
}

fn run_job(be: &mut dyn Backend, job: Job) -> Done {
    let ask = |be: &mut dyn Backend, req: Request| match be.request(&req) {
        Ok(Reply::Error { message }) => Err(message),
        Ok(reply) => Ok(reply),
        Err(_) => Err("Uzumaki isn't reachable".to_string()),
    };
    match job {
        Job::Tracks { kind, id } => {
            let result = ask(be, Request::Tracks { kind, id: id.clone() }).and_then(|r| match r {
                Reply::Tracks { collection, songs } => Ok((collection, songs)),
                _ => Err("unexpected reply".to_string()),
            });
            Done::Tracks { kind, id, result }
        }
        Job::Search { seq, query } => {
            let result = ask(be, Request::Search { query: query.clone() }).and_then(|r| match r {
                Reply::Songs { songs } => Ok(songs),
                _ => Err("unexpected reply".to_string()),
            });
            Done::Search { seq, result }
        }
        Job::Cover { key, kitty_id } => {
            let result = cover::build(&key.path, key.caps, key.cells, kitty_id);
            Done::Cover { key, kitty_id, result }
        }
    }
}

/// Song lists: one thread per job, each with its own connection. Searches: one worker, one at
/// a time, because the app keeps only its latest search for `Pick`; two searches in flight
/// could finish out of order and Enter would pick from the other one. Results come back over
/// a channel.
pub struct Threaded {
    tx: Sender<Done>,
    rx: Receiver<Done>,
    searches: Option<Sender<Job>>,
}

impl Threaded {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        Threaded { tx, rx, searches: None }
    }

    fn search_worker(&mut self) -> &Sender<Job> {
        self.searches.get_or_insert_with(|| {
            let (jobs_tx, jobs_rx) = mpsc::channel::<Job>();
            let tx = self.tx.clone();
            thread::spawn(move || {
                let mut be = Reconnecting { client: None };
                for job in jobs_rx {
                    if tx.send(run_job(&mut be, job)).is_err() {
                        break;
                    }
                }
            });
            jobs_tx
        })
    }
}

impl Jobs for Threaded {
    fn submit(&mut self, job: Job) {
        if let Job::Search { .. } = job {
            let _ = self.search_worker().send(job);
            return;
        }
        let tx = self.tx.clone();
        thread::spawn(move || {
            let done = run_job(&mut Reconnecting { client: None }, job);
            let _ = tx.send(done);
        });
    }

    fn poll(&mut self) -> Vec<Done> {
        self.rx.try_iter().collect()
    }
}

/// Runs each job at once on the calling thread (tests).
#[cfg(test)]
pub struct Inline {
    be: Box<dyn Backend>,
    done: Vec<Done>,
}

#[cfg(test)]
impl Inline {
    pub fn new(be: Box<dyn Backend>) -> Self {
        Inline { be, done: Vec::new() }
    }
}

#[cfg(test)]
impl Jobs for Inline {
    fn submit(&mut self, job: Job) {
        let done = run_job(&mut *self.be, job);
        self.done.push(done);
    }

    fn poll(&mut self) -> Vec<Done> {
        std::mem::take(&mut self.done)
    }
}
