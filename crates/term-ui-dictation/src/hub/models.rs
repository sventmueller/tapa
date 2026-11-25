//! Downloading a local model with lowspeak's model store, into the folders Lowspeak uses too, so either app finds
//! what the other downloaded.

use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use lowspeak::models::LocalModel;

use super::engine;
use crate::util::main_soon;

/// The download that runs, and the model whose last download failed.
#[derive(Default)]
pub(super) struct Downloads {
    running: Option<Running>,
    failed: Option<LocalModel>,
    changed: Option<Rc<dyn Fn()>>,
}

struct Running {
    model: LocalModel,
    fraction: f64,
    cancel: Arc<AtomicBool>,
}

enum Step {
    Progress(f64),
    Done(Result<(), engine::EngineError>),
}

fn with<R>(f: impl FnOnce(&mut Downloads) -> R) -> R {
    f(&mut super::state().models.borrow_mut())
}

/// Starts downloading `model` unless a download runs; `changed` is called on the main thread at each step.
pub fn download(model: LocalModel, changed: impl Fn() + 'static) {
    let cancel = Arc::new(AtomicBool::new(false));
    let started = with(|downloads| {
        if downloads.running.is_some() {
            return false;
        }
        downloads.running = Some(Running {
            model,
            fraction: 0.0,
            cancel: Arc::clone(&cancel),
        });
        downloads.failed = None;
        downloads.changed = Some(Rc::new(changed));
        true
    });
    if !started {
        return;
    }
    let spawned = crate::util::spawn("term.dictation-model", move || {
        let mut shown = 0.0;
        let result = engine::download(model, &cancel, &mut |fraction| {
            if fraction - shown >= 0.01 {
                shown = fraction;
                main_soon(move || step(model, Step::Progress(fraction)));
            }
        });
        main_soon(move || step(model, Step::Done(result)));
    });
    if let Err(error) = spawned {
        step(model, Step::Done(Err(engine::EngineError::Lowspeak(error.to_string()))));
    }
}

fn step(model: LocalModel, step: Step) {
    let changed = with(|downloads| {
        let running = downloads.running.as_mut().filter(|r| r.model == model)?;
        match step {
            Step::Progress(fraction) => running.fraction = fraction,
            Step::Done(result) => {
                let cancelled = running.cancel.load(Ordering::SeqCst);
                downloads.running = None;
                if let Err(error) = result
                    && !cancelled
                {
                    term_base::diag::report("dictation", format!("downloading {}: {error}", model.raw_value()));
                    downloads.failed = Some(model);
                }
            }
        }
        downloads.changed.clone()
    });
    if let Some(changed) = changed {
        changed();
    }
}

/// Stops the download that runs; the model stays missing.
pub fn cancel_download() {
    with(|downloads| {
        if let Some(running) = &downloads.running {
            running.cancel.store(true, Ordering::SeqCst);
        }
    });
}

/// The model being downloaded and how far, from 0 to 1.
pub fn downloading() -> Option<(LocalModel, f64)> {
    with(|downloads| downloads.running.as_ref().map(|r| (r.model, r.fraction)))
}

/// The model whose last download failed.
pub fn download_failed() -> Option<LocalModel> {
    with(|downloads| downloads.failed)
}

/// Whether `model` is on this Mac.
pub fn installed(model: LocalModel) -> bool {
    engine::installed_models().contains(&model)
}
