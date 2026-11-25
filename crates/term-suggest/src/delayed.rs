//! The delayed stage (E8): specs not parsed yet, the dynamic sources and the next command without input, worked off
//! in a small pool of threads. Each asker has a generation; a request makes the older ones stale, and what a stale
//! request or one over the time limit finds is dropped. What the sources found is cached for a few seconds.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::complete::{self, Candidate, Completion};
use crate::next::{self, Next};
use crate::rank::Context;
use crate::sources::Source;

/// How long the delayed stage may take before what it found is dropped.
pub const TIME_LIMIT: Duration = Duration::from_millis(500);

/// How long what a source found stays good: long enough for the keys of one word, short enough for a new branch.
const CACHE_TIME: Duration = Duration::from_secs(5);

/// The threads of the pool: a slow program on one leaves the other for files and specs.
const THREADS: usize = 2;

type Job = Box<dyn FnOnce() + Send>;

/// What each source found per folder, and when.
type Found = HashMap<(Source, PathBuf), (Instant, Vec<Candidate>)>;

struct Pool {
    jobs: Mutex<Sender<Job>>,
    generations: Mutex<HashMap<u64, u64>>,
    cache: Mutex<Found>,
}

fn pool() -> &'static Pool {
    static POOL: OnceLock<Pool> = OnceLock::new();
    POOL.get_or_init(|| {
        let (jobs, inbox) = channel::<Job>();
        let inbox = Arc::new(Mutex::new(inbox));
        for index in 0..THREADS {
            let inbox = inbox.clone();
            let _ = std::thread::Builder::new()
                .name(format!("suggest-{index}"))
                .spawn(move || {
                    loop {
                        let job = inbox.lock().ok().and_then(|inbox| inbox.recv().ok());
                        match job {
                            Some(job) => job(),
                            None => return,
                        }
                    }
                });
        }
        Pool {
            jobs: Mutex::new(jobs),
            generations: Mutex::new(HashMap::new()),
            cache: Mutex::new(HashMap::new()),
        }
    })
}

/// A new generation for `asker`, which makes its earlier requests stale.
fn begin(asker: u64) -> u64 {
    let mut generations = pool()
        .generations
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let generation = generations.entry(asker).or_insert(0);
    *generation += 1;
    *generation
}

/// Whether `generation` is still `asker`'s newest request.
fn current(asker: u64, generation: u64) -> bool {
    pool()
        .generations
        .lock()
        .is_ok_and(|g| g.get(&asker) == Some(&generation))
}

/// Makes every request of `asker` stale, as when its text changed to something the delayed stage has nothing for.
pub fn cancel(asker: u64) {
    begin(asker);
}

fn submit(job: Job) {
    if let Ok(jobs) = pool().jobs.lock() {
        let _ = jobs.send(job);
    }
}

/// Completes the last word of `line` in the pool: with the spec parsed if it was not, and with what the dynamic
/// sources find. `done` gets the completion on a pool thread, unless a newer request of `asker` came or the time limit
/// passed.
pub fn complete_later(
    asker: u64,
    line: String,
    folder: PathBuf,
    home: PathBuf,
    done: impl FnOnce(Completion) + Send + 'static,
) {
    let generation = begin(asker);
    let asked = Instant::now();
    submit(Box::new(move || {
        if !current(asker, generation) {
            return;
        }
        let completion = complete_with_sources(&line, &folder, &home, asked + TIME_LIMIT);
        if current(asker, generation) && asked.elapsed() <= TIME_LIMIT {
            done(completion);
        }
    }));
}

/// The completion of `line` with the spec and the dynamic sources, the sources given up at `deadline`.
pub fn complete_with_sources(line: &str, folder: &Path, home: &Path, deadline: Instant) -> Completion {
    let mut completion = complete::complete(line, folder, home, true);
    let Some(source) = completion.wanted.as_ref().and_then(crate::sources::source_for) else {
        return completion;
    };
    let found = cached_find(&source, folder, home, deadline);
    let word = completion.word.clone();
    let mut dynamic: Vec<Candidate> = found.into_iter().filter(|c| c.text.starts_with(&word)).collect();
    dynamic.retain(|c| !completion.candidates.iter().any(|have| have.text == c.text));
    dynamic.append(&mut completion.candidates);
    completion.candidates = dynamic;
    completion
}

fn cached_find(source: &Source, folder: &Path, home: &Path, deadline: Instant) -> Vec<Candidate> {
    let key = (source.clone(), folder.to_path_buf());
    if let Ok(cache) = pool().cache.lock()
        && let Some((at, found)) = cache.get(&key)
        && at.elapsed() < CACHE_TIME
    {
        return found.clone();
    }
    let found = source.find(folder, home, deadline);
    if let Ok(mut cache) = pool().cache.lock() {
        cache.retain(|_, (at, _)| at.elapsed() < CACHE_TIME);
        if !found.is_empty() || source.reads_files() {
            cache.insert(key, (Instant::now(), found.clone()));
        }
    }
    found
}

/// The next command without input (F210) beyond the history, worked out in the pool; `done` gets it unless a newer
/// request of `asker` came or the time limit passed.
pub fn next_later(asker: u64, context: Context, done: impl FnOnce(Option<Next>) + Send + 'static) {
    let generation = begin(asker);
    let asked = Instant::now();
    submit(Box::new(move || {
        if !current(asker, generation) {
            return;
        }
        let next = next::next_without_input(&context);
        if current(asker, generation) && asked.elapsed() <= TIME_LIMIT {
            done(next);
        }
    }));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn the_newest_request_answers_and_stale_ones_are_dropped() {
        let dir = term_base::testing::scratch("suggest-delayed");
        std::fs::create_dir_all(dir.join(".git/refs/heads")).unwrap();
        std::fs::write(dir.join(".git/refs/heads/main"), "x").unwrap();
        std::fs::write(dir.join(".git/refs/heads/topic"), "x").unwrap();
        let (tx, rx) = mpsc::channel();
        let stale = tx.clone();
        let asker = 9_001;
        complete_later(
            asker,
            "git checkout m".into(),
            dir.to_path_buf(),
            dir.to_path_buf(),
            move |c| {
                let _ = stale.send(("stale", c));
            },
        );
        complete_later(
            asker,
            "git checkout t".into(),
            dir.to_path_buf(),
            dir.to_path_buf(),
            move |c| {
                let _ = tx.send(("newest", c));
            },
        );
        let (which, completion) = rx.recv_timeout(Duration::from_secs(5)).expect("an answer");
        assert_eq!(which, "newest");
        assert_eq!(completion.candidates[0].text, "topic");
        assert!(
            rx.recv_timeout(Duration::from_millis(200)).is_err(),
            "the stale one is dropped"
        );
    }

    #[test]
    fn the_next_command_comes_from_the_pool() {
        let dir = term_base::testing::scratch("suggest-delayed-next");
        std::fs::write(dir.join("pnpm-lock.yaml"), "").unwrap();
        let (tx, rx) = mpsc::channel();
        let context = Context {
            folder: dir.to_path_buf(),
            ..Context::default()
        };
        next_later(9_002, context, move |next| {
            let _ = tx.send(next);
        });
        let next = rx.recv_timeout(Duration::from_secs(5)).expect("an answer");
        assert_eq!(next.map(|n| n.line).as_deref(), Some("pnpm install"));
    }
}
