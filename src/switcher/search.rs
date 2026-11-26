//! Quick open: an index of each workspace's files and a snapshot of open tabs.

use std::cmp::Reverse;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use objc2_app_kit::NSWindow;
use splat_i18n::t;
use term_base::{Cancel, LockIgnoringPoison, main_soon};
use term_ui::palette::{self, Entry, Group, RankQuery, RankText};
use term_ui::workspace_host::WorkspaceId;

use super::{Destination, entry, places, windows_front_first};

struct Root {
    workspace: WorkspaceId,
    path: PathBuf,
    name: String,
}

struct Candidate {
    entry: Entry,
    destination: Destination,
    icon: term_ui_icons::Icon,
    searchable: RankText,
}

impl Candidate {
    fn new(entry: Entry, destination: Destination, icon: term_ui_icons::Icon) -> Self {
        let title = entry.title.as_str();
        let name = match destination {
            Destination::File(..) => title.rsplit('/').next().unwrap_or(title),
            _ => title,
        };
        let searchable = RankText::new(name, &entry.hidden);
        Self {
            entry,
            destination,
            icon,
            searchable,
        }
    }
}

struct Catalog {
    title: Option<String>,
    entries: Vec<Candidate>,
}

struct Index {
    catalogs: Vec<Option<Arc<Catalog>>>,
    remaining: usize,
}

#[derive(Default)]
struct Requests {
    query: String,
    latest: Option<(u64, String)>,
    working: bool,
}

struct Search {
    cancel: Cancel,
    index: Mutex<Index>,
    requests: Mutex<Requests>,
    revision: AtomicU64,
    refresh_queued: AtomicBool,
    tabs: Catalog,
}

pub(super) fn toggle(window: &NSWindow) {
    if palette::is_open() {
        palette::close();
        return;
    }
    let home = term_base::home();
    let tabs = Catalog {
        title: None,
        entries: places()
            .into_iter()
            .map(|place| Candidate::new(entry(&place, 0, &home), place.destination, place.icon))
            .collect(),
    };
    let roots = roots();
    let search = Arc::new(Search {
        cancel: Cancel::new(),
        index: Mutex::new(Index {
            catalogs: vec![None; roots.len()],
            remaining: roots.len(),
        }),
        requests: Mutex::new(Requests::default()),
        revision: AtomicU64::new(0),
        refresh_queued: AtomicBool::new(false),
        tabs,
    });
    let handle = search.clone();
    palette::open_grouped(
        window,
        t("Datei suchen"),
        search.cancel.clone(),
        Rc::new(move |query| handle.request(query)),
    );
    if !roots.is_empty() {
        let worker = search.clone();
        if term_base::thread::detach("workspace file index", move || worker.index(roots)).is_err() {
            search.index.lock_ignoring_poison().remaining = 0;
            search.refresh();
        }
    }
}

fn roots() -> Vec<Root> {
    let windows = windows_front_first();
    let several = windows.len() > 1;
    let mut roots = Vec::new();
    for (number, window) in windows.iter().enumerate() {
        if window.is_quick() {
            continue;
        }
        let selected = window.selected_workspace().map(|workspace| workspace.id);
        for workspace in window.workspaces() {
            // Remote workspaces' placeholder folders contain no project files.
            if workspace.remote_machine().is_some() {
                continue;
            }
            let mut name = workspace.name();
            if several {
                name = format!("{name} · {} {}", t("Fenster"), number + 1);
            }
            roots.push((
                (workspace.scratch, number, Some(workspace.id) != selected),
                Root {
                    workspace: WorkspaceId::new(workspace.id),
                    path: workspace.base.clone(),
                    name,
                },
            ));
        }
    }
    roots.sort_by_key(|(order, _)| *order);
    let mut counts = HashMap::new();
    for (_, root) in &roots {
        *counts.entry(root.name.clone()).or_insert(0) += 1;
    }
    let home = term_base::home();
    roots
        .into_iter()
        .map(|(_, mut root)| {
            if counts[&root.name] > 1 {
                let path = term_ui_sidebar::model::short_folder(&root.path, &home);
                root.name = format!("{} · {path}", root.name);
            }
            root
        })
        .collect()
}

impl Search {
    fn index(self: &Arc<Self>, roots: Vec<Root>) {
        // Workspaces can be on independent volumes. Keep enough cores free for the foreground and parser.
        let cores = std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get);
        let workers = roots.len().min((cores / 2).clamp(1, 4));
        let roots = Mutex::new(roots.into_iter().enumerate());
        let work = || {
            term_base::priority::utility();
            self.index_roots(&roots);
        };
        std::thread::scope(|scope| {
            for _ in 1..workers {
                if std::thread::Builder::new()
                    .name("workspace file index".into())
                    .spawn_scoped(scope, work)
                    .is_err()
                {
                    break;
                }
            }
            work();
        });
    }

    fn index_roots(self: &Arc<Self>, roots: &Mutex<impl Iterator<Item = (usize, Root)>>) {
        while !self.cancel.is_cancelled() {
            let Some((position, root)) = roots.lock_ignoring_poison().next() else {
                return;
            };
            let files = term_project::file_index(&root.path, &self.cancel);
            let entries = files
                .into_iter()
                .take_while(|_| !self.cancel.is_cancelled())
                .filter_map(|path| file(&root, path))
                .collect();
            if self.cancel.is_cancelled() {
                return;
            }
            let catalog = Catalog {
                title: Some(root.name),
                entries,
            };
            {
                let mut index = self.index.lock_ignoring_poison();
                index.catalogs[position] = Some(Arc::new(catalog));
                index.remaining -= 1;
            }
            self.refresh();
        }
    }

    fn refresh(self: &Arc<Self>) {
        if !self.refresh_queued.swap(true, Ordering::AcqRel) {
            let search = self.clone();
            main_soon(move || {
                search.refresh_queued.store(false, Ordering::Release);
                palette::refresh_grouped(&search.cancel);
            });
        }
    }

    fn request(self: &Arc<Self>, query: String) {
        if self.cancel.is_cancelled() {
            return;
        }
        {
            let mut requests = self.requests.lock_ignoring_poison();
            if requests.query != query {
                requests.query.clone_from(&query);
                self.revision.fetch_add(1, Ordering::Release);
            }
            if query.trim().is_empty() {
                requests.latest = None;
                return;
            }
            requests.latest = Some((self.revision.load(Ordering::Relaxed), query.clone()));
            if requests.working {
                return;
            }
            requests.working = true;
        }
        let worker = self.clone();
        if term_base::thread::detach("quick open matches", move || {
            term_base::priority::user_initiated();
            worker.rank();
        })
        .is_err()
        {
            let mut requests = self.requests.lock_ignoring_poison();
            requests.working = false;
            requests.latest = None;
            drop(requests);
            if let Some((groups, destinations)) = results([&self.tabs], &query, || false) {
                present(&self.cancel, &query, groups, destinations, false);
            }
        }
    }

    fn rank(self: &Arc<Self>) {
        while !self.cancel.is_cancelled() {
            let (revision, query) = {
                let mut requests = self.requests.lock_ignoring_poison();
                let Some(query) = requests.latest.take() else {
                    requests.working = false;
                    return;
                };
                query
            };
            let (index, loading) = {
                let index = self.index.lock_ignoring_poison();
                (index.catalogs.clone(), index.remaining != 0)
            };
            let catalogs = index.iter().flatten().map(Arc::as_ref).chain([&self.tabs]);
            if let Some((groups, destinations)) = results(catalogs, &query, || self.superseded(revision)) {
                let search = self.clone();
                main_soon(move || {
                    if !search.superseded(revision) {
                        present(&search.cancel, &query, groups, destinations, loading);
                    }
                });
            }
        }
    }

    fn superseded(&self, revision: u64) -> bool {
        self.cancel.is_cancelled() || self.revision.load(Ordering::Acquire) != revision
    }
}

fn file(root: &Root, path: PathBuf) -> Option<Candidate> {
    let relative = path.strip_prefix(&root.path).ok()?;
    let icon = term_ui_icons::icon_of(&relative.file_name()?.to_string_lossy());
    Some(Candidate::new(
        Entry {
            title: relative.to_string_lossy().into_owned(),
            hidden: format!("{} {}", relative.display(), root.name),
            shortcut: String::new(),
            tag: 0,
        },
        Destination::File(root.workspace, path),
        icon,
    ))
}

type Results = (Vec<Group>, Vec<(Destination, term_ui_icons::Icon)>);

/// File groups follow their best match, then keep the entries from that workspace together.
fn results<'a>(
    catalogs: impl IntoIterator<Item = &'a Catalog>,
    query: &str,
    cancelled: impl Fn() -> bool,
) -> Option<Results> {
    if query.trim().is_empty() {
        return Some((Vec::new(), Vec::new()));
    }
    let query = RankQuery::new(query);
    let mut matches = Vec::new();
    for (section, catalog) in catalogs.into_iter().enumerate() {
        let rows = query.best(catalog.entries.iter().map(|entry| &entry.searchable), 100, &cancelled)?;
        if let Some(&(score, _)) = rows.first() {
            matches.push(((Reverse(score), section), catalog, rows));
        }
    }
    matches.sort_unstable_by_key(|(order, _, _)| *order);
    let mut destinations = Vec::new();
    let groups = matches
        .into_iter()
        .map(|(_, catalog, rows)| Group {
            title: catalog.title.clone(),
            entries: rows
                .into_iter()
                .map(|(_, index)| {
                    let candidate = &catalog.entries[index];
                    let tag = destinations.len();
                    destinations.push((candidate.destination.clone(), candidate.icon));
                    Entry {
                        tag,
                        ..candidate.entry.clone()
                    }
                })
                .collect(),
        })
        .collect();
    Some((groups, destinations))
}

fn present(
    cancel: &Cancel,
    query: &str,
    groups: Vec<Group>,
    destinations: Vec<(Destination, term_ui_icons::Icon)>,
    loading: bool,
) {
    let destinations = Rc::new(destinations);
    let icons = destinations.clone();
    let chosen = Rc::new(move |tag: usize| {
        if let Some((destination, _)) = destinations.get(tag) {
            destination.clone().show();
        }
    });
    let icon: palette::ResultIcon = Rc::new(move |tag| {
        let (_, icon) = icons.get(tag)?;
        term_ui_icons::icon_image(icon, 1.0)
    });
    palette::show_grouped(cancel, query, groups, loading, chosen, icon);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalog(id: u64, name: &str, paths: &[&str]) -> Catalog {
        let root = Root {
            workspace: WorkspaceId::new(id),
            path: PathBuf::from("/projects").join(name),
            name: name.to_string(),
        };
        Catalog {
            title: Some(root.name.clone()),
            entries: paths
                .iter()
                .map(|path| file(&root, root.path.join(path)).unwrap())
                .collect(),
        }
    }

    #[test]
    fn identical_names_are_grouped_and_open_in_their_own_workspace() {
        let alpha = catalog(1, "Alpha", &["src/settings.toml"]);
        let beta = catalog(2, "Beta", &["config/settings.toml", "settings.toml.bak"]);
        let (groups, destinations) = results([&alpha, &beta], "settings.toml", || false).unwrap();
        assert_eq!(groups[0].title.as_deref(), Some("Alpha"));
        assert_eq!(groups[1].title.as_deref(), Some("Beta"));
        assert_eq!(groups[1].entries.len(), 2);
        let settings = &groups[1].entries[0];
        assert_eq!(settings.title, "config/settings.toml");
        assert_eq!(
            destinations[settings.tag].0,
            Destination::File(WorkspaceId::new(2), "/projects/Beta/config/settings.toml".into())
        );
        let (groups, _) = results([&alpha, &beta], "config/settings.toml", || false).unwrap();
        assert_eq!(groups[0].title.as_deref(), Some("Beta"));
        assert!(results([&alpha, &beta], " \t", || false).unwrap().0.is_empty());
    }

    #[test]
    fn a_large_group_keeps_its_best_hundred_in_stable_order() {
        let paths: Vec<_> = (0..150).map(|i| format!("dir{i:03}/app_copy.rs")).collect();
        let paths: Vec<_> = paths.iter().map(String::as_str).chain(["last/app.rs"]).collect();
        let alpha = catalog(1, "Alpha", &paths);
        let (groups, _) = results([&alpha], "app.rs", || false).unwrap();
        let rows = &groups[0].entries;
        assert_eq!(rows.len(), 100);
        assert_eq!(rows[0].title, "last/app.rs");
        assert_eq!(rows[1].title, "dir000/app_copy.rs");
        assert_eq!(rows[99].title, "dir098/app_copy.rs");
    }
}
