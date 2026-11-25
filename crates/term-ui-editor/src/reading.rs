//! Reading on the disk lane what an editor needs from a file: the main thread waits a frame at most, so a slow volume
//! never holds it up; what comes later is taken when it comes, and what wanted the editor of a file gets it then.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::time::Duration;

use objc2::rc::Retained;
use term_base::lane::{Answer, DISK};
use term_base::signpost;
use term_document::Document;
use term_ui::workspace_host::{Tabs, Workspace, WorkspaceId as WindowId};

use super::EditorTextView;
use super::opening::opened;

/// How long the main thread waits for a read, a frame at 120 Hz: an answer within it opens the tab at once, as
/// callers that use the editor right after opening expect.
const BUDGET: Duration = Duration::from_millis(8);

type Then = Box<dyn FnOnce(&EditorTextView)>;

/// The files whose read for a tab still runs, with what waits for their editor.
#[derive(Default)]
pub(crate) struct Reads {
    running: RefCell<Vec<Read>>,
}

struct Read {
    window: WindowId,
    path: PathBuf,
    waiting: Vec<Then>,
}

impl Reads {
    fn is_running(&self, window: WindowId, path: &Path) -> bool {
        self.running
            .borrow()
            .iter()
            .any(|r| r.window == window && r.path == path)
    }

    fn start(&self, window: WindowId, path: PathBuf) {
        self.running.borrow_mut().push(Read {
            window,
            path,
            waiting: Vec::new(),
        });
    }

    /// Queues `then` for the editor of `path` in `window` if its read runs; gives it back otherwise.
    fn wait(&self, window: WindowId, path: &Path, then: Then) -> Result<(), Then> {
        let mut running = self.running.borrow_mut();
        match running.iter_mut().find(|r| r.window == window && r.path == path) {
            Some(read) => {
                read.waiting.push(then);
                Ok(())
            }
            None => Err(then),
        }
    }

    /// Ends the read of `path` in `window`, with what waited for it.
    fn finish(&self, window: WindowId, path: &Path) -> Vec<Then> {
        let mut running = self.running.borrow_mut();
        let at = running.iter().position(|r| r.window == window && r.path == path);
        at.map(|at| running.remove(at).waiting).unwrap_or_default()
    }
}

/// Reads `path` on the disk lane and opens its tab: now if the read is quick, else once it is done. A second open
/// while the read runs starts no second one, so the file still gets one document (E24).
pub(super) fn open(controller: &Workspace, path: &Path, font_size: f64, preview: bool) {
    let window = controller.id();
    let state = crate::state();
    let reads = &state.reads;
    if reads.is_running(window, path) {
        return;
    }
    let read = path.to_path_buf();
    let answer = DISK.ask_within(BUDGET, move || {
        let _interval = signpost::begin(signpost::name!("open read"));
        Document::open(&read)
    });
    match answer {
        Ok(Answer::Now(document)) => opened(controller, path, finished(document), font_size, preview),
        Ok(Answer::Later(pending)) => {
            reads.start(window, path.to_path_buf());
            let path = path.to_path_buf();
            pending.then(move |document| {
                crate::util::main_soon(move || open_late(window, &path, finished(document), font_size, preview));
            });
        }
        Err(_) => opened(controller, path, Document::open(path), font_size, preview),
    }
}

fn finished(document: Option<std::io::Result<Document>>) -> std::io::Result<Document> {
    document.unwrap_or_else(|| Err(std::io::ErrorKind::Interrupted.into()))
}

fn open_late(window: WindowId, path: &Path, document: std::io::Result<Document>, font_size: f64, preview: bool) {
    let Some(controller) = Workspace::open(window) else {
        return;
    };
    let waiting = crate::state().reads.finish(window, path);
    if !controller.select_tab(&path.to_string_lossy()) {
        opened(&controller, path, document, font_size, preview);
    }
    if let Some(view) = editor_in(&controller, path) {
        for then in waiting {
            then(&view);
        }
    }
}

/// Runs `then` with the editor of `path` in `controller`: now, or once its tab opens after its read.
pub(crate) fn with_opened(controller: &Workspace, path: &Path, then: impl FnOnce(&EditorTextView) + 'static) {
    let state = crate::state();
    if let Err(then) = state.reads.wait(controller.id(), path, Box::new(then))
        && let Some(view) = editor_in(controller, path)
    {
        then(&view);
    }
}

fn editor_in(controller: &Workspace, path: &Path) -> Option<Retained<EditorTextView>> {
    super::editors_in(controller)
        .into_iter()
        .find(|v| v.document().path() == Some(path))
}
