//! What the fuzz targets of the agent connections check of the thread they fill.

use std::collections::{HashMap, HashSet};

use crate::model::{EntryId, Lane};
use crate::thread::Thread;
use crate::update::{Change, Update};

/// A thread with the timelines a list rebuilds from the changes alone, as the UI's virtualised list does.
pub struct Replay {
    /// The thread the updates go to.
    pub(crate) thread: Thread,
    lanes: HashMap<Lane, Vec<EntryId>>,
    removed: HashSet<EntryId>,
}

impl Replay {
    /// A replay of `thread`, which must be new.
    pub fn new(thread: Thread) -> Replay {
        Replay {
            thread,
            lanes: HashMap::new(),
            removed: HashSet::new(),
        }
    }

    /// Applies `update`, then checks that the changes it reported rebuild every timeline, that the thread's lookups
    /// agree with its timelines, and that no id comes back once removed.
    pub fn apply(&mut self, update: Update) {
        let next = self.thread.next;
        for change in self.thread.apply(update) {
            match change {
                Change::Inserted { lane, id, index } => {
                    assert!(id.0 >= next && !self.removed.contains(&id), "{id:?} again");
                    let list = self.lanes.entry(lane).or_default();
                    assert!(index <= list.len(), "{id:?} inserted at {index} of {}", list.len());
                    list.insert(index, id);
                }
                Change::Removed { lane, id } => {
                    let list = self.lanes.entry(lane).or_default();
                    let at = list.iter().position(|known| *known == id);
                    list.remove(at.unwrap_or_else(|| panic!("{id:?} removed but never inserted")));
                    self.removed.insert(id);
                }
                Change::Updated { lane, id } => {
                    assert!(
                        self.lanes.get(&lane).is_some_and(|list| list.contains(&id)),
                        "{id:?} updated"
                    );
                }
                Change::Cleared => {
                    self.removed.extend(self.lanes.values().flatten().copied());
                    self.lanes.clear();
                }
                Change::Header(_) | Change::Usage | Change::Todos | Change::Subagent(_) | Change::Background => {}
            }
        }
        self.check();
    }

    fn check(&self) {
        let thread = &self.thread;
        let mut lanes: Vec<Lane> = vec![Lane::Main];
        lanes.extend(thread.lanes.keys().cloned().map(Lane::Subagent));
        lanes.extend(self.lanes.keys().cloned());
        let mut entries = 0;
        for lane in &lanes {
            let ids: Vec<EntryId> = thread.entries(lane).iter().map(|entry| entry.id).collect();
            let rebuilt = self.lanes.get(lane).map_or(&[][..], Vec::as_slice);
            assert_eq!(ids, rebuilt, "{lane:?}");
            for (index, id) in ids.iter().enumerate() {
                assert_eq!(thread.place(*id), Some((lane, index)), "{id:?}");
                assert!(id.0 < thread.next);
            }
        }
        for lane in thread.lanes.keys() {
            entries += thread.entries(&Lane::Subagent(lane.clone())).len();
        }
        entries += thread.main.len();
        assert_eq!(thread.places.len(), entries, "places of entries that are gone");
        for (key, id) in thread.keys.iter() {
            assert!(thread.entry(*id).is_some(), "{key} names {id:?}, which is gone");
        }
        let agents: HashSet<&str> = thread.subagents.iter().map(|agent| agent.key.as_str()).collect();
        let timelines: HashSet<&str> = thread.lanes.keys().map(String::as_str).collect();
        assert_eq!(agents.len(), thread.subagents.len(), "a subagent twice");
        assert_eq!(agents, timelines, "subagents and their timelines");
        assert!(thread.running <= thread.background.len());
    }
}
