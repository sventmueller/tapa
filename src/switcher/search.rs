//! Quick open: an index of each workspace's files and a snapshot of open tabs.

use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use objc2_app_kit::NSWindow;
use splat_i18n::t;
use term_base::{Cancel, LockIgnoringPoison, main_soon};
use term_ui::palette::{self, Entry, Group};
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
}

struct Catalog {
    title: Option<String>,
    entries: Vec<Candidate>,
}

#[derive(Default)]
struct Index {
    catalogs: Vec<Arc<Catalog>>,
    complete: bool,
}

#[derive(Default)]
struct Requests {
    latest: Option<String>,
    working: bool,
}

struct Search {
    cancel: Cancel,
    index: Mutex<Index>,
    requests: Mutex<Requests>,
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
            .map(|place| Candidate {
                entry: entry(&place, 0, &home),
                destination: place.destination,
                icon: place.icon,
            })
            .collect(),
    };
    let search = Arc::new(Search {
        cancel: Cancel::new(),
        index: Mutex::new(Index::default()),
        requests: Mutex::new(Requests::default()),
        tabs,
    });
    let handle = search.clone();
    palette::open_grouped(
        window,
        t("Datei suchen"),
        search.cancel.clone(),
        Rc::new(move |query| handle.request(query)),
    );
    let roots = roots();
    let worker = search.clone();
    if term_base::thread::detach("workspace file index", move || worker.index(roots)).is_err() {
        search.finish_index();
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
    fn index(&self, roots: Vec<Root>) {
        for root in roots {
            let files = term_project::file_index(&root.path, &self.cancel);
            if self.cancel.is_cancelled() {
                return;
            }
            let entries = files.into_iter().filter_map(|path| file(&root, path)).collect();
            let catalog = Catalog {
                title: Some(root.name),
                entries,
            };
            self.index.lock_ignoring_poison().catalogs.push(Arc::new(catalog));
            self.refresh();
        }
        self.finish_index();
    }

    fn finish_index(&self) {
        self.index.lock_ignoring_poison().complete = true;
        self.refresh();
    }

    fn refresh(&self) {
        let cancel = self.cancel.clone();
        main_soon(move || palette::refresh_grouped(&cancel));
    }

    fn request(self: &Arc<Self>, query: String) {
        if self.cancel.is_cancelled() {
            return;
        }
        {
            let mut requests = self.requests.lock_ignoring_poison();
            requests.latest = Some(query.clone());
            if requests.working {
                return;
            }
            requests.working = true;
        }
        let worker = self.clone();
        if term_base::thread::detach("quick open matches", move || worker.rank()).is_err() {
            self.requests.lock_ignoring_poison().working = false;
            let (groups, destinations) = results(&[&self.tabs], &query);
            present(&self.cancel, &query, groups, destinations, false);
        }
    }

    fn rank(&self) {
        while !self.cancel.is_cancelled() {
            let query = {
                let mut requests = self.requests.lock_ignoring_poison();
                let Some(query) = requests.latest.take() else {
                    requests.working = false;
                    return;
                };
                query
            };
            let (index, loading) = {
                let index = self.index.lock_ignoring_poison();
                (index.catalogs.clone(), !index.complete)
            };
            let catalogs: Vec<_> = index.iter().map(Arc::as_ref).chain([&self.tabs]).collect();
            let (groups, destinations) = results(&catalogs, &query);
            let cancel = self.cancel.clone();
            main_soon(move || present(&cancel, &query, groups, destinations, loading));
        }
    }
}

fn file(root: &Root, path: PathBuf) -> Option<Candidate> {
    let relative = path.strip_prefix(&root.path).ok()?;
    let icon = term_ui_icons::icon_of(&relative.file_name()?.to_string_lossy());
    Some(Candidate {
        entry: Entry {
            title: relative.to_string_lossy().into_owned(),
            hidden: format!("{} {}", relative.display(), root.name),
            shortcut: String::new(),
            tag: 0,
        },
        destination: Destination::File(root.workspace, path),
        icon,
    })
}

/// File groups follow their best match, then keep the entries from that workspace together.
fn results(catalogs: &[&Catalog], query: &str) -> (Vec<Group>, Vec<(Destination, term_ui_icons::Icon)>) {
    if query.trim().is_empty() {
        return (Vec::new(), Vec::new());
    }
    let candidates: Vec<_> = catalogs
        .iter()
        .enumerate()
        .flat_map(|(section, catalog)| catalog.entries.iter().map(move |candidate| (section, candidate)))
        .collect();
    let texts: Vec<_> = candidates
        .iter()
        .map(|(_, candidate)| {
            let title = candidate.entry.title.as_str();
            let name = match candidate.destination {
                Destination::File(..) => title.rsplit('/').next().unwrap_or(title),
                _ => title,
            };
            (name, candidate.entry.hidden.as_str())
        })
        .collect();
    let mut order = Vec::new();
    let mut matches = vec![Vec::new(); catalogs.len()];
    for index in palette::rank(query, &texts) {
        let (section, candidate) = candidates[index];
        let rows = &mut matches[section];
        if rows.is_empty() {
            order.push(section);
        }
        // Bound the number of results materialized on the main thread.
        if rows.len() < 100 {
            rows.push(candidate);
        }
    }
    let mut destinations = Vec::new();
    let groups = order
        .into_iter()
        .map(|section| Group {
            title: catalogs[section].title.clone(),
            entries: matches[section]
                .iter()
                .map(|candidate| {
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
    (groups, destinations)
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
        let (groups, destinations) = results(&[&alpha, &beta], "settings.toml");
        assert_eq!(groups[0].title.as_deref(), Some("Alpha"));
        assert_eq!(groups[1].title.as_deref(), Some("Beta"));
        assert_eq!(groups[1].entries.len(), 2);
        let settings = &groups[1].entries[0];
        assert_eq!(settings.title, "config/settings.toml");
        assert_eq!(
            destinations[settings.tag].0,
            Destination::File(WorkspaceId::new(2), "/projects/Beta/config/settings.toml".into())
        );
        let (groups, _) = results(&[&alpha, &beta], "config/settings.toml");
        assert_eq!(groups[0].title.as_deref(), Some("Beta"));
        assert!(results(&[&alpha, &beta], " \t").0.is_empty());
    }
}
