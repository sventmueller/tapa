//! The rules Claude stored per project with "Immer erlauben", as Dango's settings list them: read from each project's
//! `.claude` settings files and removed one at a time straight from the file, so the settings window runs no program
//! in a project folder (E16). ACP agents keep theirs where the app cannot reach them.

use std::cell::{OnceCell, RefCell};
use std::path::PathBuf;

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSAccessibility, NSButton, NSControlTextEditingDelegate, NSScrollView, NSTableColumn, NSTableView,
    NSTableViewDataSource, NSTableViewDelegate, NSTextField, NSUserInterfaceLayoutOrientation, NSView,
};
use objc2_foundation::{NSArray, NSNotification, NSObject, NSObjectProtocol};
use splat_i18n::t;
use term_base::guarded;
use term_claude::rules::{self, Scope};
use term_ui::kit::{ns, set_action, space};

/// The list's height, for about five rules.
const HEIGHT: f64 = 120.0;
/// The widths of the project's and the rule's column; the last takes the rest.
const COLUMNS: [f64; 2] = [190.0, 190.0];
/// The list's width.
const WIDTH: f64 = 520.0;

/// One rule of the list.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Row {
    project: PathBuf,
    scope: Scope,
    rule: String,
}

/// The folders whose rules the list shows: those where Claude stored one from the app, the open workspaces' and the
/// recent ones, each once.
fn folders() -> Vec<PathBuf> {
    let open: Vec<PathBuf> = crate::state::try_app()
        .map(|app| {
            app.windows
                .borrow()
                .iter()
                .flat_map(|window| window.workspaces())
                .map(|workspace| workspace.base.clone())
                .collect()
        })
        .unwrap_or_default();
    once_each(
        crate::agent::rule_folders()
            .into_iter()
            .chain(open)
            .chain(crate::recent::folders()),
    )
}

fn once_each(folders: impl IntoIterator<Item = PathBuf>) -> Vec<PathBuf> {
    let mut distinct: Vec<PathBuf> = Vec::new();
    for folder in folders {
        if !distinct.contains(&folder) {
            distinct.push(folder);
        }
    }
    distinct
}

fn rows() -> Vec<Row> {
    folders()
        .into_iter()
        .flat_map(|project| {
            rules::allow_rules(&project).into_iter().map(move |stored| Row {
                project: project.clone(),
                scope: stored.scope,
                rule: stored.rule,
            })
        })
        .collect()
}

fn scope_name(scope: Scope) -> &'static str {
    match scope {
        Scope::Local => t("dieses Projekt, nur für dich"),
        Scope::Project => t("dieses Projekt"),
    }
}

/// What the list keeps.
pub(crate) struct Ivars {
    rows: RefCell<Vec<Row>>,
    table: OnceCell<Retained<NSTableView>>,
    remove: OnceCell<Retained<NSButton>>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements and `Rules` does not implement Drop.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "TermSettingsRules"]
    #[ivars = Ivars]
    pub(crate) struct Rules;

    unsafe impl NSObjectProtocol for Rules {}

    unsafe impl NSControlTextEditingDelegate for Rules {}

    unsafe impl NSTableViewDataSource for Rules {
        #[unsafe(method(numberOfRowsInTableView:))]
        fn row_count(&self, _table: &NSTableView) -> isize {
            guarded(0, || isize::try_from(self.ivars().rows.borrow().len()).unwrap_or(0))
        }
    }

    unsafe impl NSTableViewDelegate for Rules {
        #[unsafe(method_id(tableView:viewForTableColumn:row:))]
        fn view_for(&self, _table: &NSTableView, column: Option<&NSTableColumn>, row: isize) -> Option<Retained<NSView>> {
            guarded(None, || {
                let column: usize = column?.identifier().to_string().parse().ok()?;
                let text = self.cell(usize::try_from(row).ok()?, column)?;
                let cell = NSTextField::labelWithString(&ns(&text), self.mtm());
                cell.setLineBreakMode(objc2_app_kit::NSLineBreakMode::ByTruncatingMiddle);
                cell.setToolTip(Some(&ns(&text)));
                Some(Retained::into_super(Retained::into_super(cell)))
            })
        }

        #[unsafe(method(tableViewSelectionDidChange:))]
        fn selection_changed(&self, _notification: &NSNotification) {
            guarded((), || self.update_button());
        }
    }

    impl Rules {
        #[unsafe(method(termSettingsRemoveRule:))]
        fn remove_action(&self, _sender: Option<&AnyObject>) {
            guarded((), || self.remove_selected());
        }
    }
);

impl Rules {
    pub(crate) fn new(mtm: MainThreadMarker) -> Retained<Rules> {
        let this = Self::alloc(mtm).set_ivars(Ivars {
            rows: RefCell::new(Vec::new()),
            table: OnceCell::new(),
            remove: OnceCell::new(),
        });
        // SAFETY: NSObject's designated initializer, called once on the newly allocated object.
        unsafe { msg_send![super(this), init] }
    }

    /// The list with its button and what it explains.
    pub(crate) fn view(&self) -> Retained<NSView> {
        let mtm = self.mtm();
        let table = NSTableView::new(mtm);
        table.setUsesAlternatingRowBackgroundColors(true);
        table.setAccessibilityLabel(Some(&ns(t("Gespeicherte Regeln von Claude"))));
        let titles = [t("Projekt"), t("Regel"), t("Gilt für")];
        for (at, title) in titles.into_iter().enumerate() {
            let column = NSTableColumn::initWithIdentifier(NSTableColumn::alloc(mtm), &ns(&at.to_string()));
            column.setTitle(&ns(title));
            if let Some(width) = COLUMNS.get(at) {
                column.setWidth(*width);
            }
            table.addTableColumn(&column);
        }
        // SAFETY: The table holds its data source and delegate weakly; the settings window keeps the list as long.
        unsafe {
            table.setDataSource(Some(objc2::runtime::ProtocolObject::from_ref(self)));
            table.setDelegate(Some(objc2::runtime::ProtocolObject::from_ref(self)));
        }
        let scroll = NSScrollView::new(mtm);
        scroll.setDocumentView(Some(&table));
        scroll.setHasVerticalScroller(true);
        scroll.setBorderType(objc2_app_kit::NSBorderType::BezelBorder);
        scroll.setTranslatesAutoresizingMaskIntoConstraints(false);
        term_ui::kit::activate([
            scroll.heightAnchor().constraintEqualToConstant(HEIGHT),
            scroll.widthAnchor().constraintEqualToConstant(WIDTH),
        ]);
        // SAFETY: A nil target and no action until `set_action` gives it both.
        let remove = unsafe { NSButton::buttonWithTitle_target_action(&ns(t("Regel entfernen")), None, None, mtm) };
        set_action(&remove, self, sel!(termSettingsRemoveRule:));
        let note = super::form::secondary(
            mtm,
            t(
                "Was du Claude mit „Immer erlauben“ für ein Projekt freigegeben hast. ACP-Agenten speichern ihre Regeln selbst, wo die App sie weder sieht noch löschen kann.",
            ),
        );
        let views: [Retained<NSView>; 3] = [
            Retained::into_super(scroll),
            Retained::into_super(Retained::into_super(remove.clone())),
            Retained::into_super(Retained::into_super(note)),
        ];
        let stack = objc2_app_kit::NSStackView::stackViewWithViews(&NSArray::from_retained_slice(&views), mtm);
        stack.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
        stack.setAlignment(objc2_app_kit::NSLayoutAttribute::Leading);
        stack.setSpacing(space::M);
        let _ = self.ivars().table.set(table);
        let _ = self.ivars().remove.set(remove);
        self.reload();
        Retained::into_super(stack)
    }

    /// Reads the rules again from the projects' files and shows them.
    pub(crate) fn reload(&self) {
        *self.ivars().rows.borrow_mut() = rows();
        if let Some(table) = self.ivars().table.get() {
            table.reloadData();
        }
        self.update_button();
    }

    fn cell(&self, row: usize, column: usize) -> Option<String> {
        let rows = self.ivars().rows.borrow();
        let row = rows.get(row)?;
        Some(match column {
            0 => term_ui::kit::panel::display_path(&row.project),
            1 => row.rule.clone(),
            _ => scope_name(row.scope).to_string(),
        })
    }

    fn selected(&self) -> Option<Row> {
        let row = usize::try_from(self.ivars().table.get()?.selectedRow()).ok()?;
        self.ivars().rows.borrow().get(row).cloned()
    }

    fn update_button(&self) {
        if let Some(remove) = self.ivars().remove.get() {
            remove.setEnabled(self.selected().is_some());
        }
    }

    fn remove_selected(&self) {
        let Some(row) = self.selected() else { return };
        if let Err(error) = rules::remove_allow_rule(&row.project, row.scope, &row.rule) {
            let file = term_ui::kit::panel::display_path(&row.scope.file(&row.project));
            term_ui::kit::alert::warn(
                self.mtm(),
                t("Die Regel ließ sich nicht entfernen."),
                &format!("{file}\n{error}"),
            );
        }
        self.reload();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_folder_is_listed_once_where_it_first_appears() {
        let folders = ["/b", "/a", "/b", "/c", "/a"].map(PathBuf::from);
        assert_eq!(once_each(folders), ["/b", "/a", "/c"].map(PathBuf::from));
    }
}
