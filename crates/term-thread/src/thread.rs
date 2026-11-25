//! A thread and the one way to change it.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use crate::model::{
    Activity, BackgroundItem, Decision, Entry, EntryId, EntryKind, Lane, RateLimit, RequestState, Rewind, Status,
    Subagent, TaskStatus, Todo, ToolView, Usage,
};
use crate::update::{BackgroundPatch, Change, Field, SubagentPatch, ToolPatch, Update, UsagePatch};

#[cfg(feature = "fuzzing")]
#[doc(hidden)]
pub mod fuzzing;

/// The longest title a first message gives; the views cut it further to their width.
const TITLE_LIMIT: usize = 200;

/// How many ended background items a thread keeps for its list, the latest ones.
const KEPT_ENDED: usize = 50;

/// A thread's title from the first message `text`: its first line that is not blank, with an ellipsis when cut at
/// [`TITLE_LIMIT`] characters; empty for a message without text.
pub fn title_from(text: &str) -> String {
    let line = text.lines().find(|l| !l.trim().is_empty()).unwrap_or_default().trim();
    let mut title: String = line.chars().take(TITLE_LIMIT).collect();
    if title.len() < line.len() {
        title.truncate(title.trim_end().len());
        title.push('…');
    }
    title
}

/// A conversation with one agent: its head, its timeline, its subagents and its background work.
///
/// Entries and lookup indexes are shared; copying a thread copies the entry pointers but no index keys. The UI
/// can keep a snapshot while the connection goes on changing the original.
#[derive(Debug, Clone)]
pub struct Thread {
    id: crate::model::ThreadId,
    title: String,
    agent: String,
    cwd: PathBuf,
    session: Option<String>,
    status: Status,
    activity: Option<Activity>,
    mode: Option<String>,
    model: Option<String>,
    effort: Option<String>,
    usage: Usage,
    rate_limits: Vec<RateLimit>,
    todos: Vec<Todo>,
    main: Vec<Arc<Entry>>,
    subagents: Vec<Subagent>,
    lanes: HashMap<String, Vec<Arc<Entry>>>,
    /// The items that run, in the agent's order, then those that ended, in the order they ended.
    background: Vec<BackgroundItem>,
    /// How many items at the start of `background` run.
    running: usize,
    next: u64,
    keys: Arc<HashMap<String, EntryId>>,
    places: Arc<HashMap<EntryId, (Lane, usize)>>,
}

impl Thread {
    /// An empty thread with `agent` in `cwd`.
    pub fn new(id: crate::model::ThreadId, agent: impl Into<String>, cwd: impl Into<PathBuf>) -> Thread {
        Thread {
            id,
            title: String::new(),
            agent: agent.into(),
            cwd: cwd.into(),
            session: None,
            status: Status::Idle,
            activity: None,
            mode: None,
            model: None,
            effort: None,
            usage: Usage::default(),
            rate_limits: Vec::new(),
            todos: Vec::new(),
            main: Vec::new(),
            subagents: Vec::new(),
            lanes: HashMap::new(),
            background: Vec::new(),
            running: 0,
            next: 1,
            keys: Arc::default(),
            places: Arc::default(),
        }
    }

    /// The app's id for the thread.
    pub fn id(&self) -> &crate::model::ThreadId {
        &self.id
    }

    /// The title; empty until one is set.
    pub fn title(&self) -> &str {
        &self.title
    }

    /// The agent's id in the register, such as `claude`.
    pub fn agent(&self) -> &str {
        &self.agent
    }

    /// The folder the agent works in.
    pub fn cwd(&self) -> &Path {
        &self.cwd
    }

    /// The agent's own id for the conversation.
    pub fn session(&self) -> Option<&str> {
        self.session.as_deref()
    }

    /// Whether the agent works, waits or rests.
    pub fn status(&self) -> Status {
        self.status
    }

    /// What the agent is busy with beyond its turn.
    pub fn activity(&self) -> Option<Activity> {
        self.activity
    }

    /// The agent's mode.
    pub fn mode(&self) -> Option<&str> {
        self.mode.as_deref()
    }

    /// The model.
    pub fn model(&self) -> Option<&str> {
        self.model.as_deref()
    }

    /// The reasoning effort.
    pub fn effort(&self) -> Option<&str> {
        self.effort.as_deref()
    }

    /// Tokens, cost and context.
    pub fn usage(&self) -> &Usage {
        &self.usage
    }

    /// The plan's usage windows as last reported, one per window.
    pub fn rate_limits(&self) -> &[RateLimit] {
        &self.rate_limits
    }

    /// The agent's current to-do list.
    pub fn todos(&self) -> &[Todo] {
        &self.todos
    }

    /// The entries of `lane` in order; empty for a lane that does not exist.
    pub fn entries(&self, lane: &Lane) -> &[Arc<Entry>] {
        match lane {
            Lane::Main => &self.main,
            Lane::Subagent(key) => self.lanes.get(key).map_or(&[], Vec::as_slice),
        }
    }

    /// The entry with `id`, in whichever lane it is.
    pub fn entry(&self, id: EntryId) -> Option<&Entry> {
        let (lane, index) = self.places.get(&id)?;
        self.entries(lane).get(*index).map(Arc::as_ref)
    }

    /// The entry the connection named `key`.
    pub fn entry_by_key(&self, key: &str) -> Option<&Entry> {
        self.entry(*self.keys.get(key)?)
    }

    /// The lane and position of the entry with `id`.
    pub fn place(&self, id: EntryId) -> Option<(&Lane, usize)> {
        self.places.get(&id).map(|(lane, index)| (lane, *index))
    }

    /// The subagents in the order they started.
    pub fn subagents(&self) -> &[Subagent] {
        &self.subagents
    }

    /// The subagent with `key`.
    pub fn subagent(&self, key: &str) -> Option<&Subagent> {
        self.subagents.iter().find(|s| s.key == key)
    }

    /// The work the agent keeps running in the background, then the latest of what ended.
    pub fn background(&self) -> &[BackgroundItem] {
        &self.background
    }

    /// The agent's id for the frame that resuming up to and including the entry `id` names: its own, else that of
    /// the nearest earlier entry of the main timeline that has one; none in a subagent's lane.
    pub fn resume_point(&self, id: EntryId) -> Option<&str> {
        let (Lane::Main, index) = self.places.get(&id)? else {
            return None;
        };
        self.main[..=*index].iter().rev().find_map(|e| e.uuid.as_deref())
    }

    /// How to rewind the conversation to just before the prompt `id`; none for other entries, for subagent lanes and
    /// for the first prompt, before which a new session starts instead.
    pub fn rewind_before(&self, id: EntryId) -> Option<Rewind> {
        let (Lane::Main, _) = self.places.get(&id)? else {
            return None;
        };
        let entry = self.entry(id)?;
        let EntryKind::UserMessage(_) = entry.kind else {
            return None;
        };
        Some(Rewind {
            fork_at: entry.follows.clone()?,
            drops_turn: entry.uuid.clone()?,
        })
    }

    /// Permission requests, questions and plan approvals that wait for the user, in all lanes.
    pub fn pending_requests(&self) -> Vec<&Entry> {
        std::iter::once(&self.main)
            .chain(self.lanes.values())
            .flatten()
            .map(Arc::as_ref)
            .filter(|e| request_state(&e.kind) == Some(&RequestState::Pending))
            .collect()
    }

    /// Applies every update in turn and returns all that changed.
    pub fn apply_all(&mut self, updates: impl IntoIterator<Item = Update>) -> Vec<Change> {
        updates.into_iter().flat_map(|update| self.apply(update)).collect()
    }

    /// Applies `update` and returns what changed; an update that names an unknown entry changes nothing.
    ///
    /// ```
    /// use std::time::SystemTime;
    ///
    /// use term_thread::{AgentMessage, Change, EntryKind, Lane, Thread, ThreadId, Update};
    ///
    /// let mut thread = Thread::new(ThreadId::from("t1"), "claude", "/work");
    /// let message = EntryKind::AgentMessage(AgentMessage { markdown: "Hel".into(), streaming: true });
    /// let put = Update::Put { lane: Lane::Main, key: Some("m1".into()), at: SystemTime::now(), entry: message };
    /// let [Change::Inserted { id, index: 0, .. }] = thread.apply(put)[..] else { panic!("not inserted") };
    /// let appended = thread.apply(Update::Append { key: "m1".into(), text: "lo".into() });
    /// assert_eq!(appended, [Change::Updated { lane: Lane::Main, id }]);
    /// assert!(matches!(&thread.entry(id).unwrap().kind, EntryKind::AgentMessage(m) if m.markdown == "Hello"));
    /// assert!(thread.apply(Update::Append { key: "unknown".into(), text: "!".into() }).is_empty());
    /// ```
    pub fn apply(&mut self, update: Update) -> Vec<Change> {
        let mut changes = Vec::new();
        match update {
            Update::Session(session) => set(&mut self.session, Some(session), Field::Session, &mut changes),
            Update::Title(title) => set(&mut self.title, title, Field::Title, &mut changes),
            Update::Status(status) => set(&mut self.status, status, Field::Status, &mut changes),
            Update::Activity(activity) => set(&mut self.activity, activity, Field::Activity, &mut changes),
            Update::Mode(mode) => set(&mut self.mode, Some(mode), Field::Mode, &mut changes),
            Update::Model(model) => set(&mut self.model, Some(model), Field::Model, &mut changes),
            Update::Effort(effort) => set(&mut self.effort, effort, Field::Effort, &mut changes),
            Update::Usage(mut patch) => {
                if let Some(turn) = patch.turn.as_mut().filter(|t| t.message.is_none()) {
                    turn.message = self.last_user_message();
                }
                if patch_usage(&mut self.usage, patch) {
                    changes.push(Change::Usage);
                }
            }
            Update::RateLimit(limit) => self.rate_limit(limit, &mut changes),
            Update::Todos(todos) => {
                if self.todos != todos {
                    self.todos = todos;
                    changes.push(Change::Todos);
                }
            }
            Update::Put { lane, key, at, entry } => self.put(lane, key, at, entry, &mut changes),
            Update::Append { key, text } => self.append(&key, &text, &mut changes),
            Update::Finish { key, text, at } => self.finish(&key, text, at, &mut changes),
            Update::Tool { key, patch } => self.tool(&key, patch, &mut changes),
            Update::Resolve { key, state } => self.edit(&key, &mut changes, |kind| match kind {
                EntryKind::PermissionRequest(r) => {
                    // A denied request keeps its input, by which the live order tests tell what a rule refused.
                    if matches!(
                        state,
                        RequestState::Decided(Decision::Allowed { .. }) | RequestState::Withdrawn
                    ) {
                        r.input = serde_json::Value::Null;
                    }
                    replace(&mut r.state, state)
                }
                EntryKind::Question(q) => replace(&mut q.state, state),
                EntryKind::PlanApproval(p) => replace(&mut p.state, state),
                _ => false,
            }),
            Update::Ask { key, request_id } => self.edit(&key, &mut changes, |kind| match kind {
                EntryKind::Question(q) => {
                    replace(&mut q.request_id, Some(request_id)) | replace(&mut q.state, RequestState::Pending)
                }
                EntryKind::PlanApproval(p) => {
                    replace(&mut p.request_id, Some(request_id)) | replace(&mut p.state, RequestState::Pending)
                }
                _ => false,
            }),
            Update::Chain { key, uuid, follows } => {
                if let Some(id) = self.keys.get(&key).copied() {
                    self.edit_entry(id, &mut changes, |entry| {
                        let mut changed = replace(&mut entry.uuid, Some(uuid));
                        if entry.follows.is_none() && follows.is_some() {
                            entry.follows = follows;
                            changed = true;
                        }
                        changed
                    });
                }
            }
            Update::Remove { key } => self.remove(&key, &mut changes),
            Update::Subagent(patch) => self.subagent_patch(patch, &mut changes),
            Update::Background(items) => self.replace_background(items, &mut changes),
            Update::BackgroundItem(patch) => self.background_patch(patch, &mut changes),
            Update::Clear => self.clear(&mut changes),
        }
        changes
    }

    /// The user message of the main timeline that came last.
    fn last_user_message(&self) -> Option<EntryId> {
        self.main
            .iter()
            .rev()
            .find(|e| matches!(e.kind, EntryKind::UserMessage(_)))
            .map(|e| e.id)
    }

    fn rate_limit(&mut self, limit: RateLimit, changes: &mut Vec<Change>) {
        match self.rate_limits.iter_mut().find(|l| l.window == limit.window) {
            Some(known) if *known == limit => return,
            Some(known) => *known = limit,
            None => self.rate_limits.push(limit),
        }
        changes.push(Change::Usage);
    }

    fn put(
        &mut self,
        lane: Lane,
        key: Option<String>,
        at: SystemTime,
        mut entry: EntryKind,
        changes: &mut Vec<Change>,
    ) {
        if let EntryKind::ToolCall(call) = &mut entry {
            call.settle();
        }
        if let Some(id) = key.as_ref().and_then(|k| self.keys.get(k)).copied() {
            self.edit_id(id, changes, |kind| replace(kind, entry));
            return;
        }
        if let Lane::Subagent(owner) = &lane
            && !self.subagents.iter().any(|s| &s.key == owner)
        {
            self.subagent_patch(SubagentPatch::new(owner.clone(), at), changes);
        }
        let id = EntryId(self.next);
        self.next += 1;
        let timeline = match &lane {
            Lane::Main => &mut self.main,
            Lane::Subagent(owner) => self.lanes.entry(owner.clone()).or_default(),
        };
        let index = timeline.len();
        timeline.push(Arc::new(Entry {
            id,
            at,
            kind: entry,
            uuid: None,
            follows: None,
        }));
        Arc::make_mut(&mut self.places).insert(id, (lane.clone(), index));
        if let Some(key) = key {
            Arc::make_mut(&mut self.keys).insert(key, id);
        }
        changes.push(Change::Inserted { lane, id, index });
    }

    fn append(&mut self, key: &str, text: &str, changes: &mut Vec<Change>) {
        if text.is_empty() {
            return;
        }
        self.edit(key, changes, |kind| match kind {
            EntryKind::AgentMessage(m) => {
                m.markdown.push_str(text);
                m.streaming = true;
                true
            }
            EntryKind::Thought(t) => {
                t.text.push_str(text);
                t.streaming = true;
                true
            }
            _ => false,
        });
    }

    fn finish(&mut self, key: &str, text: Option<String>, at: SystemTime, changes: &mut Vec<Change>) {
        let Some(id) = self.keys.get(key).copied() else { return };
        let began = self.entry(id).map(|e| e.at);
        self.edit_id(id, changes, |kind| match kind {
            EntryKind::AgentMessage(m) => {
                let mut changed = replace(&mut m.streaming, false);
                if let Some(text) = text {
                    changed |= replace(&mut m.markdown, text);
                }
                changed
            }
            EntryKind::Thought(t) => {
                let mut changed = replace(&mut t.streaming, false);
                if let Some(text) = text.filter(|text| !text.is_empty() || t.text.is_empty()) {
                    changed |= replace(&mut t.text, text);
                }
                if t.duration.is_none() {
                    t.duration = began.and_then(|began| at.duration_since(began).ok());
                    changed = true;
                }
                changed
            }
            _ => false,
        });
    }

    fn tool(&mut self, key: &str, patch: ToolPatch, changes: &mut Vec<Change>) {
        self.edit(key, changes, |kind| {
            let EntryKind::ToolCall(call) = kind else { return false };
            let mut changed = false;
            if let Some(input) = patch.input {
                changed |= replace(&mut call.input, input);
            }
            if let Some(title) = patch.title {
                changed |= replace(&mut call.title, title);
            }
            if let Some(view) = patch.view {
                let results = match &call.view {
                    ToolView::Search { results, .. } => *results,
                    _ => None,
                };
                changed |= replace(&mut call.view, view);
                if let (ToolView::Search { results: slot, .. }, Some(known)) = (&mut call.view, results) {
                    slot.get_or_insert(known);
                }
            }
            if let Some(status) = patch.status {
                changed |= replace(&mut call.status, status);
            }
            if let Some(output) = patch.output {
                changed |= replace(&mut call.output, Some(output));
                call.left_out = 0;
                call.changes_left_out = (0, 0);
            }
            if call.started.is_none() && patch.started.is_some() {
                call.started = patch.started;
                changed = true;
            }
            if let Some(ended) = patch.ended {
                changed |= replace(&mut call.ended, Some(ended));
                if patch.elapsed.is_none()
                    && let Some(elapsed) = call.started.and_then(|s| ended.duration_since(s).ok())
                {
                    changed |= replace(&mut call.elapsed, Some(elapsed));
                }
            }
            if let Some(elapsed) = patch.elapsed {
                changed |= replace(&mut call.elapsed, Some(elapsed));
            }
            if let (Some(n), ToolView::Search { results, .. }) = (patch.results, &mut call.view) {
                changed |= replace(results, Some(n));
            }
            if let Some(summary) = patch.summary {
                changed |= replace(&mut call.summary, Some(summary));
            }
            if let Some(task) = patch.background_task {
                changed |= replace(&mut call.background_task, Some(task));
            }
            call.settle();
            changed
        });
    }

    fn remove(&mut self, key: &str, changes: &mut Vec<Change>) {
        let Some(id) = Arc::make_mut(&mut self.keys).remove(key) else {
            return;
        };
        Arc::make_mut(&mut self.keys).retain(|_, other| *other != id);
        let Some((lane, index)) = Arc::make_mut(&mut self.places).remove(&id) else {
            return;
        };
        let timeline = match &lane {
            Lane::Main => &mut self.main,
            Lane::Subagent(owner) => match self.lanes.get_mut(owner) {
                Some(timeline) => timeline,
                None => return,
            },
        };
        if index < timeline.len() {
            timeline.remove(index);
            for (offset, entry) in timeline.iter().enumerate().skip(index) {
                Arc::make_mut(&mut self.places).insert(entry.id, (lane.clone(), offset));
            }
        }
        changes.push(Change::Removed { lane, id });
    }

    fn subagent_patch(&mut self, patch: SubagentPatch, changes: &mut Vec<Change>) {
        let (index, created) = match self.subagents.iter().position(|s| s.key == patch.key) {
            Some(index) => (index, false),
            None => {
                self.subagents.push(Subagent {
                    key: patch.key.clone(),
                    task_id: None,
                    agent_type: None,
                    description: String::new(),
                    status: TaskStatus::Running,
                    started: patch.at,
                    ended: None,
                    elapsed: std::time::Duration::ZERO,
                    tokens: 0,
                    tool_uses: 0,
                    last_tool: None,
                    summary: None,
                    cancellable: false,
                    backgrounded: false,
                    parent: None,
                });
                self.lanes.entry(patch.key.clone()).or_default();
                (self.subagents.len() - 1, true)
            }
        };
        let before = self.subagents[index].clone();
        let agent = &mut self.subagents[index];
        let SubagentPatch {
            key,
            at: _,
            task_id,
            agent_type,
            description,
            status,
            ended,
            elapsed,
            tokens,
            tool_uses,
            last_tool,
            summary,
            cancellable,
            backgrounded,
            parent,
        } = patch;
        merge(&mut agent.task_id, task_id);
        merge(&mut agent.agent_type, agent_type);
        if let Some(description) = description {
            agent.description = description;
        }
        if let Some(status) = status {
            agent.status = status;
        }
        merge(&mut agent.ended, ended);
        if let Some(elapsed) = elapsed {
            agent.elapsed = elapsed;
        }
        if let Some(tokens) = tokens {
            agent.tokens = tokens;
        }
        if let Some(tool_uses) = tool_uses {
            agent.tool_uses = tool_uses;
        }
        merge(&mut agent.last_tool, last_tool);
        merge(&mut agent.summary, summary);
        if let Some(cancellable) = cancellable {
            agent.cancellable = cancellable;
        }
        if let Some(backgrounded) = backgrounded {
            agent.backgrounded = backgrounded;
        }
        merge(&mut agent.parent, parent);
        if created || *agent != before {
            changes.push(Change::Subagent(key));
        }
    }

    fn replace_background(&mut self, items: Vec<BackgroundItem>, changes: &mut Vec<Change>) {
        let mut merged: Vec<BackgroundItem> = items
            .into_iter()
            .map(|mut item| {
                if let Some(known) = self.background.iter().find(|b| b.id == item.id) {
                    merge(&mut item.started, known.started);
                    merge(&mut item.tool_call, known.tool_call.clone());
                    merge(&mut item.parent, known.parent.clone());
                    merge_tail(&mut item.output, &known.output);
                }
                item
            })
            .collect();
        let (running, ended) = self.background.split_at(self.running.min(self.background.len()));
        let just_ended = running.iter().cloned().map(|mut item| {
            if !item.status.is_final() {
                item.status = TaskStatus::Completed;
            }
            item
        });
        let ended: Vec<BackgroundItem> = ended
            .iter()
            .cloned()
            .chain(just_ended)
            .filter(|item| !merged.iter().any(|other| other.id == item.id))
            .collect();
        let now_running = merged.len();
        let excess = ended.len().saturating_sub(KEPT_ENDED);
        merged.extend(ended.into_iter().skip(excess));
        self.running = now_running;
        if merged != self.background {
            self.background = merged;
            changes.push(Change::Background);
        }
    }

    fn background_patch(&mut self, patch: BackgroundPatch, changes: &mut Vec<Change>) {
        let Some(item) = self.background.iter_mut().find(|b| b.id == patch.id) else {
            return;
        };
        let before = item.clone();
        if let Some(status) = patch.status {
            item.status = status;
        }
        if item.ended.is_none() {
            item.ended = patch.ended;
        }
        if let Some(output) = patch.output {
            let mut next = output;
            merge_tail(&mut next, &item.output);
            item.output = next;
        }
        if let Some(backgrounded) = patch.is_backgrounded {
            item.is_backgrounded = backgrounded;
        }
        if *item != before {
            changes.push(Change::Background);
        }
    }

    fn clear(&mut self, changes: &mut Vec<Change>) {
        self.main.clear();
        self.subagents.clear();
        self.lanes.clear();
        self.background.clear();
        self.running = 0;
        self.todos.clear();
        self.usage = Usage::default();
        self.keys = Arc::default();
        self.places = Arc::default();
        changes.push(Change::Cleared);
    }

    fn edit(&mut self, key: &str, changes: &mut Vec<Change>, f: impl FnOnce(&mut EntryKind) -> bool) {
        if let Some(id) = self.keys.get(key).copied() {
            self.edit_id(id, changes, f);
        }
    }

    fn edit_id(&mut self, id: EntryId, changes: &mut Vec<Change>, f: impl FnOnce(&mut EntryKind) -> bool) {
        self.edit_entry(id, changes, |entry| f(&mut entry.kind));
    }

    fn edit_entry(&mut self, id: EntryId, changes: &mut Vec<Change>, f: impl FnOnce(&mut Entry) -> bool) {
        let Some((lane, index)) = self.places.get(&id).cloned() else {
            return;
        };
        let timeline = match &lane {
            Lane::Main => &mut self.main,
            Lane::Subagent(owner) => match self.lanes.get_mut(owner) {
                Some(timeline) => timeline,
                None => return,
            },
        };
        let Some(slot) = timeline.get_mut(index) else { return };
        if f(Arc::make_mut(slot)) {
            changes.push(Change::Updated { lane, id });
        }
    }
}

impl EntryKind {
    /// Whether this is a request that waits for the user's answer.
    pub fn waits_for_answer(&self) -> bool {
        request_state(self) == Some(&RequestState::Pending)
    }
}

fn request_state(kind: &EntryKind) -> Option<&RequestState> {
    match kind {
        EntryKind::PermissionRequest(r) => Some(&r.state),
        EntryKind::Question(q) => Some(&q.state),
        EntryKind::PlanApproval(p) => Some(&p.state),
        _ => None,
    }
}

fn set<T: PartialEq>(slot: &mut T, value: T, field: Field, changes: &mut Vec<Change>) {
    if replace(slot, value) {
        changes.push(Change::Header(field));
    }
}

/// Puts `value` into `slot` and tells whether that changed it.
fn replace<T: PartialEq>(slot: &mut T, value: T) -> bool {
    if *slot == value {
        false
    } else {
        *slot = value;
        true
    }
}

fn merge<T>(slot: &mut Option<T>, value: Option<T>) {
    if value.is_some() {
        *slot = value;
    }
}

fn merge_tail(next: &mut crate::model::OutputTail, known: &crate::model::OutputTail) {
    if next.path.is_none() {
        next.path.clone_from(&known.path);
    }
    if next.text.is_none() {
        next.text.clone_from(&known.text);
        next.truncated = known.truncated;
    }
    if next.total_bytes.is_none() {
        next.total_bytes = known.total_bytes;
    }
}

fn patch_usage(usage: &mut Usage, patch: UsagePatch) -> bool {
    let before = usage.clone();
    let UsagePatch {
        input_tokens,
        output_tokens,
        cache_read_tokens,
        cache_write_tokens,
        cost_usd,
        reported_cost,
        reported_tokens,
        context_used,
        context_size,
        turn,
        models,
        breakdown,
        subscription,
    } = patch;
    usage.turns.extend(turn);
    if let Some(subscription) = subscription {
        usage.subscription = subscription;
    }
    if let Some(models) = models {
        usage.models = models;
    }
    if breakdown.is_some() {
        usage.breakdown = breakdown;
    }
    if let Some(n) = input_tokens {
        usage.input_tokens = n;
    }
    if let Some(n) = output_tokens {
        usage.output_tokens = n;
    }
    if let Some(n) = cache_read_tokens {
        usage.cache_read_tokens = n;
    }
    if let Some(n) = cache_write_tokens {
        usage.cache_write_tokens = n;
    }
    merge(&mut usage.cost_usd, cost_usd);
    if let Some(cost) = reported_cost {
        if cost.currency != "USD" {
            usage.cost_usd = None;
        }
        usage.reported_cost = Some(cost);
    }
    if reported_tokens.is_some() {
        usage.reported_tokens = reported_tokens;
    }
    merge(&mut usage.context_used, context_used);
    merge(&mut usage.context_size, context_size);
    *usage != before
}
