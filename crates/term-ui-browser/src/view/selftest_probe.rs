//! What the self-test reads from and does with the parts of `view.rs`, kept out of its production code.

#[allow(
    unused_imports,
    reason = "the parts moved here use only some of their module's names"
)]
use super::*;

impl BrowserView {
    pub(crate) fn address_field(&self) -> Retained<objc2_app_kit::NSTextField> {
        self.parts().controls.field.clone()
    }

    pub(crate) fn seen(&self) -> tabs::Seen {
        let parts = self.parts();
        tabs::Seen {
            url: self.current_url(),
            address: parts.controls.field.stringValue().to_string(),
            editing: parts.controls.field.currentEditor().is_some(),
            inspector: web::has_inspector(&parts.web),
            failure: parts.failure.is_shown().then(|| parts.failure.heading()),
            proceed: parts.failure.offers_proceed(),
            notice: parts.notice.text(),
        }
    }

    /// Hands `done` the page's title, address and text, read in the app's own script world, which runs also where
    /// the page's scripts may not.
    pub(crate) fn read_page(&self, done: impl FnOnce(String) + 'static) {
        let parts = self.parts();
        let head = format!(
            "title {:?} url {:?}",
            web::state(&parts.web).title,
            self.current_url().unwrap_or_default()
        );
        let done = Cell::new(Some(done));
        let handler = block2::RcBlock::new(move |result: *mut AnyObject, _error: *mut NSError| {
            guarded((), || {
                // SAFETY: WebKit hands over the script's result, or nil, valid for the duration of the call.
                let text = unsafe { result.as_ref() }
                    .and_then(|value| value.downcast_ref::<NSString>().map(|text| text.to_string()))
                    .unwrap_or_default();
                if let Some(done) = done.take() {
                    done(format!("{head} text {text:?}"));
                }
            });
        });
        // SAFETY: A script without arguments in the main frame; the client world keeps the page's scripts out of it.
        unsafe {
            parts
                .web
                .callAsyncJavaScript_arguments_inFrame_inContentWorld_completionHandler(
                    &ns("return document.body ? document.body.innerText : ''"),
                    None,
                    None,
                    &objc2_web_kit::WKContentWorld::defaultClientWorld(self.mtm()),
                    Some(&handler),
                );
        }
    }
}
