//! The row above the input field's text (E7): the folder the shell reported and the branch of the
//! repository it is in, read from the files in `.git` (E16).

use std::path::Path;

use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::{
    NSColor, NSFont, NSImageView, NSLayoutAttribute, NSLineBreakMode, NSStackView, NSTextField,
    NSUserInterfaceLayoutOrientation, NSView,
};
use splat_i18n::t;

use crate::kit::{label, ns, symbol_at};

/// The height of a chip.
pub(super) const HEIGHT: f64 = 18.0;

/// What the chips say for the folder `cwd`, with `home` as `~`, and the branch, if any.
pub(crate) fn texts(cwd: &Path, home: &Path, branch: Option<String>) -> (String, Option<String>) {
    let folder = crate::blocks::model::folder_text(cwd, home);
    (folder, branch.filter(|branch| !branch.is_empty()))
}

/// A symbol and a short grey text.
struct Chip {
    root: Retained<NSStackView>,
    text: Retained<NSTextField>,
}

impl Chip {
    fn new(mtm: MainThreadMarker, symbol: &str, description: &str) -> Chip {
        let root = NSStackView::new(mtm);
        root.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
        root.setAlignment(NSLayoutAttribute::CenterY);
        root.setSpacing(crate::kit::space::XS);
        let font = NSFont::systemFontOfSize(NSFont::smallSystemFontSize());
        if let Some(image) = symbol_at(symbol, description, NSFont::smallSystemFontSize() - 1.0) {
            let icon = NSImageView::imageViewWithImage(&image, mtm);
            icon.setContentTintColor(Some(&NSColor::secondaryLabelColor()));
            root.addArrangedSubview(&icon);
        }
        let text = label(mtm, "");
        text.setFont(Some(&font));
        text.setTextColor(Some(&NSColor::secondaryLabelColor()));
        text.setLineBreakMode(NSLineBreakMode::ByTruncatingHead);
        text.setContentCompressionResistancePriority_forOrientation(
            250.0,
            objc2_app_kit::NSLayoutConstraintOrientation::Horizontal,
        );
        root.addArrangedSubview(&text);
        root.setTranslatesAutoresizingMaskIntoConstraints(false);
        root.heightAnchor().constraintEqualToConstant(HEIGHT).setActive(true);
        Chip { root, text }
    }

    fn show(&self, text: Option<&str>) {
        self.root.setHidden(text.is_none());
        self.text.setStringValue(&ns(text.unwrap_or_default()));
    }
}

/// The row of chips.
pub(super) struct Chips {
    row: Retained<NSStackView>,
    folder: Chip,
    branch: Chip,
}

impl Chips {
    pub(super) fn new(mtm: MainThreadMarker) -> Chips {
        let row = NSStackView::new(mtm);
        row.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
        row.setAlignment(NSLayoutAttribute::CenterY);
        row.setSpacing(crate::kit::space::XL);
        row.setDetachesHiddenViews(true);
        let folder = Chip::new(mtm, "folder", t("Ordner"));
        let branch = Chip::new(mtm, "arrow.triangle.branch", t("Git-Branch"));
        row.addArrangedSubview(&folder.root);
        row.addArrangedSubview(&branch.root);
        let spacer = NSView::new(mtm);
        spacer.setContentHuggingPriority_forOrientation(1.0, objc2_app_kit::NSLayoutConstraintOrientation::Horizontal);
        row.addArrangedSubview(&spacer);
        Chips { row, folder, branch }
    }

    pub(super) fn view(&self) -> &NSView {
        &self.row
    }

    /// Shows the folder and the branch, the branch chip only where there is one.
    pub(super) fn show(&self, folder: &str, branch: Option<&str>) {
        self.folder.show(Some(folder));
        self.branch.show(branch);
    }

    /// What the chips say now, folder first.
    pub(super) fn texts(&self) -> Vec<String> {
        [&self.folder, &self.branch]
            .into_iter()
            .filter(|chip| !chip.root.isHidden())
            .map(|chip| chip.text.stringValue().to_string())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::texts;

    #[test]
    fn the_folder_starts_at_home_and_the_branch_chip_needs_a_branch() {
        let home = Path::new("/Users/me");
        assert_eq!(texts(home, home, None), ("~".to_string(), None));
        assert_eq!(
            texts(Path::new("/Users/me/Repos/tapa"), home, Some("main".into())),
            ("~/Repos/tapa".to_string(), Some("main".to_string()))
        );
        assert_eq!(
            texts(Path::new("/tmp"), home, Some(String::new())),
            ("/tmp".to_string(), None)
        );
    }
}
