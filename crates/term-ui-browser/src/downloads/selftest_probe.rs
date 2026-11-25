//! The self-test's hooks of `downloads.rs`.

use super::*;

impl Downloads {
    /// Saves into `Downloads` of the self-test's home, since nobody is there to answer a save panel.
    pub(crate) fn save_without_asking(
        &self,
        download: &WKDownload,
        suggested: &str,
        handler: &RcBlock<dyn Fn(*mut NSURL)>,
    ) {
        let folder = term_base::home().join("Downloads");
        let _ = std::fs::create_dir_all(&folder);
        let file = NSURL::fileURLWithPath(&ns(&folder.join(suggested).to_string_lossy()));
        self.ivars()
            .running
            .borrow_mut()
            .push((objc2::Message::retain(download), file.clone()));
        handler.call((Retained::as_ptr(&file).cast_mut(),));
    }
}
