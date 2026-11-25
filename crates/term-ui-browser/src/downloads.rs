//! Downloads from browser tabs: each asks where to save with a save panel, and the saved file is marked as coming
//! from the web, so Gatekeeper checks it before it first opens.

use std::cell::{OnceCell, RefCell};

use block2::{DynBlock, RcBlock};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, ProtocolObject};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{NSModalResponse, NSModalResponseOK, NSSavePanel};
use objc2_foundation::{
    NSData, NSDictionary, NSError, NSFileManager, NSObjectProtocol, NSSearchPathDirectory, NSSearchPathDomainMask,
    NSString, NSURL, NSURLResponse,
};
use objc2_web_kit::{WKDownload, WKDownloadDelegate};
use splat_i18n::t;
use term_ui::kit::alert::Alert;

use term_render::constant;
use term_ui::kit::{guarded, ns};

#[derive(Default)]
pub(crate) struct Ivars {
    /// The downloads under way, with the file each writes to.
    running: RefCell<Vec<(Retained<WKDownload>, Retained<NSURL>)>>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements and `Downloads` does not implement Drop.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "TermBrowserDownloads"]
    #[ivars = Ivars]
    pub(crate) struct Downloads;

    // SAFETY: NSObjectProtocol asks nothing of a subclass of NSObject.
    unsafe impl NSObjectProtocol for Downloads {}

    // SAFETY: Each method below has its selector's signature and calls every completion handler exactly once.
    unsafe impl WKDownloadDelegate for Downloads {
        #[unsafe(method(download:decideDestinationUsingResponse:suggestedFilename:completionHandler:))]
        fn decide_destination(
            &self,
            download: &WKDownload,
            _response: &NSURLResponse,
            suggested: &NSString,
            handler: &DynBlock<dyn Fn(*mut NSURL)>,
        ) {
            let handler = handler.copy();
            let fallback = handler.clone();
            let finished = guarded(false, || {
                self.ask_destination(download, &suggested.to_string(), handler);
                true
            });
            if !finished {
                fallback.call((std::ptr::null_mut(),));
            }
        }

        #[unsafe(method(downloadDidFinish:))]
        fn did_finish(&self, download: &WKDownload) {
            guarded((), || {
                if let Some(file) = self.take(download) {
                    mark_as_downloaded(download, &file);
                    // SAFETY: The download is valid during the delegate call that passed it.
                    let tab = unsafe { download.webView() }.and_then(|web| super::tabs::tab_of(&web));
                    if let Some(tab) = tab {
                        tab.downloaded(&file);
                    }
                }
            });
        }

        #[unsafe(method(download:didFailWithError:resumeData:))]
        fn did_fail(&self, download: &WKDownload, error: &NSError, _resume: Option<&NSData>) {
            guarded((), || {
                self.take(download);
                let cancelled = error.domain().to_string() == "NSURLErrorDomain" && error.code() == -999;
                if !cancelled {
                    report_failure(download, error);
                }
            });
        }
    }
);

/// The delegate of every download with the downloads that run.
#[derive(Default)]
pub(crate) struct Downloading {
    downloads: OnceCell<Retained<Downloads>>,
}

fn with_downloads<R>(f: impl FnOnce(&OnceCell<Retained<Downloads>>) -> R) -> R {
    f(&super::host::state().downloads.downloads)
}

/// The delegate of every download, which lives as long as the app, so a download outlives the tab it started in.
pub(crate) fn delegate(mtm: MainThreadMarker) -> Retained<ProtocolObject<dyn WKDownloadDelegate>> {
    let downloads = with_downloads(|cell| {
        cell.get_or_init(|| {
            let this = Downloads::alloc(mtm).set_ivars(Ivars::default());
            // SAFETY: NSObject's initializer, called once on the newly allocated object.
            unsafe { msg_send![super(this), init] }
        })
        .clone()
    });
    ProtocolObject::from_retained(downloads)
}

impl Downloads {
    /// Shows the save panel, as a sheet on the tab's window while it has one, and hands WebKit the chosen file or
    /// nothing, which cancels the download.
    fn ask_destination(&self, download: &WKDownload, suggested: &str, handler: RcBlock<dyn Fn(*mut NSURL)>) {
        #[cfg(feature = "selftest")]
        if std::env::var_os("TAPA_SELFTEST").is_some() {
            self.save_without_asking(download, suggested, &handler);
            return;
        }
        let mtm = self.mtm();
        let panel = NSSavePanel::savePanel(mtm);
        panel.setNameFieldStringValue(&ns(suggested));
        panel.setCanCreateDirectories(true);
        let folders = NSFileManager::defaultManager().URLsForDirectory_inDomains(
            NSSearchPathDirectory::DownloadsDirectory,
            NSSearchPathDomainMask::UserDomainMask,
        );
        panel.setDirectoryURL(folders.firstObject().as_deref());
        // SAFETY: The download's view is valid while WebKit asks about the download.
        let window = unsafe { download_window(download) };
        let this = objc2::rc::Weak::from(self);
        let download = objc2::Message::retain(download);
        let chosen = panel.clone();
        let done = RcBlock::new(move |response: NSModalResponse| {
            guarded((), || {
                // WebKit waits for its completion handler, so it is called even when choosing the file panics.
                let destination = guarded(None, || {
                    let file = (response == NSModalResponseOK).then(|| chosen.URL()).flatten()?;
                    if file.checkResourceIsReachableAndReturnError().is_ok() {
                        // The panel asked before replacing; WebKit writes only to a file that does not exist yet.
                        NSFileManager::defaultManager()
                            .trashItemAtURL_resultingItemURL_error(&file, None)
                            .ok()?;
                    }
                    if let Some(this) = this.load() {
                        this.ivars().running.borrow_mut().push((download.clone(), file.clone()));
                    }
                    Some(file)
                });
                let pointer = destination
                    .as_ref()
                    .map_or(std::ptr::null_mut(), |file| Retained::as_ptr(file).cast_mut());
                handler.call((pointer,));
            });
        });
        match window {
            Some(window) => panel.beginSheetModalForWindow_completionHandler(&window, &done),
            None => {
                let response = panel.runModal();
                done.call((response,));
            }
        }
    }

    /// Forgets `download`, and returns the file it wrote.
    fn take(&self, download: &WKDownload) -> Option<Retained<NSURL>> {
        let mut running = self.ivars().running.borrow_mut();
        let index = running.iter().position(|(d, _)| std::ptr::eq(&**d, download))?;
        Some(running.remove(index).1)
    }
}

/// The window of the tab a download started in, if it is still open.
///
/// # Safety
/// `download` must be one WebKit handed to a delegate method.
unsafe fn download_window(download: &WKDownload) -> Option<Retained<objc2_app_kit::NSWindow>> {
    // SAFETY: The caller passes a download from WebKit, whose view is nil or alive.
    unsafe { download.webView() }.and_then(|web| web.window())
}

/// Marks `file` as downloaded from the web, as browsers do: macOS then checks it before it first opens.
fn mark_as_downloaded(download: &WKDownload, file: &NSURL) {
    // SAFETY: The download is valid during the delegate call that passed it.
    let source = unsafe { download.originalRequest() }.and_then(|request| request.URL());
    let mut keys: Vec<Retained<NSString>> = vec![ns("LSQuarantineAgentName"), ns("LSQuarantineType")];
    let mut values: Vec<Retained<AnyObject>> = vec![
        object(ns(&term_base::identity().name)),
        object(ns("LSQuarantineTypeWebDownload")),
    ];
    if let Some(source) = source {
        keys.push(ns("LSQuarantineDataURL"));
        values.push(object(source));
    }
    let key_refs: Vec<&NSString> = keys.iter().map(|k| &**k).collect();
    let properties = NSDictionary::from_retained_objects(&key_refs, &values);
    let properties: &AnyObject = &properties;
    let key = constant!(objc2_foundation::NSURLQuarantinePropertiesKey);
    // SAFETY: The quarantine properties take a dictionary with the keys of LSQuarantine.h, which these strings are.
    let marked = unsafe { file.setResourceValue_forKey_error(Some(properties), key) };
    if let Err(error) = marked {
        term_base::diag::report("marking a download", error.code());
    }
}

fn object<T: objc2::ClassType + 'static>(value: Retained<T>) -> Retained<AnyObject> {
    value.into()
}

fn report_failure(download: &WKDownload, error: &NSError) {
    // SAFETY: The download is valid during the delegate call that passed it.
    let Some(window) = (unsafe { download_window(download) }) else {
        term_base::diag::report("a download failed", error.code());
        return;
    };
    Alert::new(window.mtm(), t("Download fehlgeschlagen"))
        .informative(&error.localizedDescription().to_string())
        .sheet(&window, |_| {});
}

#[cfg(feature = "selftest")]
mod selftest_probe;
