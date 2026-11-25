//! The pieces an overlay's list is made of, the same in every overlay: a section's head, a name with its
//! value, a line of text, a waiting line, and buttons whose action the open overlay keeps.

use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::{NSAccessibility, NSButton, NSColor, NSView};
use splat_i18n::t;

use super::Open;
use crate::tab::AgentTab;
use crate::ui;

/// The width of the name column of name-and-value rows.
const NAME_WIDTH: f64 = 150.0;
/// The width a wrapping text gets in the card.
pub(crate) const TEXT_WIDTH: f64 = 500.0;
/// How wide the text of a row with one button beside it wraps.
pub(crate) const BESIDE_ONE: f64 = 380.0;
/// How wide the text of a row with two buttons beside it wraps.
pub(crate) const BESIDE_TWO: f64 = 290.0;
/// How wide a value in the value column wraps.
pub(crate) const VALUE_WIDTH: f64 = TEXT_WIDTH - NAME_WIDTH;

/// The head of a section.
pub(crate) fn section(mtm: MainThreadMarker, title: &str) -> Retained<NSView> {
    crate::ui::view(&crate::agent_panel::section_label(mtm, title))
}

/// A row with `name` and its `value`; without a name the value is a sentence across the row.
pub(crate) fn pair(mtm: MainThreadMarker, name: Option<&str>, value: &str) -> Retained<NSView> {
    let value_view = ui::wrapping(
        mtm,
        value,
        &crate::style::body(),
        &NSColor::labelColor(),
        TEXT_WIDTH - NAME_WIDTH,
    );
    let Some(name) = name else {
        return crate::ui::view(&value_view);
    };
    let name_view = ui::label(mtm, name, &crate::style::body(), &NSColor::secondaryLabelColor());
    ui::fixed_width(&name_view, NAME_WIDTH);
    crate::ui::view(&ui::row(mtm, 8.0, &[&name_view, &value_view]))
}

/// A line of text in the secondary colour.
pub(crate) fn text(mtm: MainThreadMarker, text: &str) -> Retained<NSView> {
    let view = ui::wrapping(
        mtm,
        text,
        &crate::style::body(),
        &NSColor::secondaryLabelColor(),
        TEXT_WIDTH,
    );
    crate::ui::view(&view)
}

/// A spinner and "Lädt …", while an answer is on its way.
pub(crate) fn waiting(mtm: MainThreadMarker) -> Retained<NSView> {
    let spinner = ui::spinner(mtm);
    let label = ui::label(mtm, t("Lädt …"), &crate::style::body(), &NSColor::secondaryLabelColor());
    crate::ui::view(&ui::row(mtm, 8.0, &[&spinner, &label]))
}

/// A button titled `title` that runs `act` on the tab.
pub(crate) fn button(
    mtm: MainThreadMarker,
    open: &Open,
    title: &str,
    act: impl Fn(&AgentTab) + 'static,
) -> Retained<NSButton> {
    let tag = open.action(act);
    ui::button(mtm, title, open.frame.target(), objc2::sel!(ideDialogAction:), tag)
}

/// Shows `why` where the list was, for an answer that did not come; with "Im Terminal öffnen" when the agent's CLI
/// could do it in the terminal.
pub(crate) fn failed(tab: &AgentTab, open: &Open, why: &str) {
    let mtm = tab.mtm;
    open.frame.set_rows(&[text(mtm, why)]);
    let terminal = button(mtm, open, t("Im Terminal öffnen"), |tab| {
        crate::dialogs::open_terminal(tab);
    });
    open.frame.set_buttons(&[crate::ui::view(&terminal)]);
}

/// Greys `control` out with `why` as its tooltip when the agent cannot do its action.
pub(crate) fn block(control: &objc2_app_kit::NSControl, why: super::model::Blocked) {
    if let Some(why) = why {
        control.setEnabled(false);
        control.setToolTip(Some(&crate::common::util::ns(why)));
    }
}

/// A switch that runs `act` with its new state, greyed with `blocked`'s reason when the agent cannot.
pub(crate) fn switch(
    mtm: MainThreadMarker,
    open: &Open,
    on: bool,
    label: &str,
    blocked: super::model::Blocked,
    act: impl Fn(&AgentTab, bool) + 'static,
) -> Retained<objc2_app_kit::NSSwitch> {
    let switch = objc2_app_kit::NSSwitch::new(mtm);
    switch.setState(if on { 1 } else { 0 });
    switch.setControlSize(objc2_app_kit::NSControlSize::Small);
    crate::common::util::set_label(&switch, Some(label));
    let read = switch.clone();
    let tag = open.action(move |tab| act(tab, read.state() != 0));
    crate::common::util::set_action(&switch, open.frame.target(), objc2::sel!(ideDialogAction:));
    switch.setTag(tag);
    block(&switch, blocked);
    switch
}

/// A text field kept under `key`, for the form's values.
pub(crate) fn field(mtm: MainThreadMarker, open: &Open, key: &'static str, placeholder: &str) -> Retained<NSView> {
    let field = objc2_app_kit::NSTextField::new(mtm);
    field.setPlaceholderString(Some(&crate::common::util::ns(placeholder)));
    crate::common::util::set_label(&field, Some(placeholder));
    field.setTranslatesAutoresizingMaskIntoConstraints(false);
    open.keep_field(key, &field);
    ui::view(&field)
}

/// A pop-up of `items` kept under `key`; an item with a reason is greyed out with it.
pub(crate) fn choice(
    mtm: MainThreadMarker,
    open: &Open,
    key: &'static str,
    label: &str,
    items: &[(&str, super::model::Blocked)],
) -> Retained<NSView> {
    let popup = objc2_app_kit::NSPopUpButton::new(mtm);
    popup.setAutoenablesItems(false);
    for (title, blocked) in items {
        popup.addItemWithTitle(&crate::common::util::ns(title));
        if let (Some(why), Some(item)) = (blocked, popup.lastItem()) {
            item.setEnabled(false);
            item.setToolTip(Some(&crate::common::util::ns(why)));
        }
    }
    crate::common::util::set_label(&popup, Some(label));
    open.keep_field(key, &popup);
    ui::view(&popup)
}

/// A name and its control on one row, the name in the name column.
pub(crate) fn labelled(mtm: MainThreadMarker, name: &str, control: &NSView) -> Retained<NSView> {
    let name_view = ui::label(mtm, name, &crate::style::body(), &NSColor::secondaryLabelColor());
    ui::fixed_width(&name_view, NAME_WIDTH);
    control.setContentHuggingPriority_forOrientation(1.0, objc2_app_kit::NSLayoutConstraintOrientation::Horizontal);
    ui::view(&ui::row(mtm, 8.0, &[&name_view, control]))
}

/// A dot in `colour` before a row, telling its state.
pub(crate) fn dot(mtm: MainThreadMarker, colour: &NSColor, state: &str) -> Retained<NSView> {
    let icon = ui::icon(mtm, "circle.fill", state, 8.0, colour);
    icon.setAccessibilityElement(true);
    icon.setAccessibilityLabel(Some(&crate::common::util::ns(state)));
    ui::view(&icon)
}

/// Asks `question` with `yes` as the first button and "Abbrechen"; whether the user chose `yes`.
pub(crate) fn confirm(mtm: MainThreadMarker, question: &str, yes: &str) -> bool {
    confirm_showing(mtm, question, "", yes)
}

/// Asks `question` with `detail` below it, with `yes` and Cancel; whether the user chose `yes`.
pub(crate) fn confirm_showing(mtm: MainThreadMarker, question: &str, detail: &str, yes: &str) -> bool {
    let alert = objc2_app_kit::NSAlert::new(mtm);
    alert.setMessageText(&crate::common::util::ns(question));
    alert.setInformativeText(&crate::common::util::ns(detail));
    alert.addButtonWithTitle(&crate::common::util::ns(yes));
    alert.addButtonWithTitle(&crate::common::util::ns(t("Abbrechen")));
    alert.runModal() == objc2_app_kit::NSAlertFirstButtonReturn
}

/// A title over smaller detail lines, all wrapping at `width` so none is cut short.
pub(crate) fn lines(mtm: MainThreadMarker, width: f64, title: &str, bold: bool, details: &[&str]) -> Retained<NSView> {
    let font = if bold {
        crate::style::bold(13.0)
    } else {
        crate::style::body()
    };
    let mut views = vec![ui::view(&ui::wrapping(
        mtm,
        title,
        &font,
        &NSColor::labelColor(),
        width,
    ))];
    for detail in details.iter().filter(|d| !d.is_empty()) {
        let line = ui::wrapping(
            mtm,
            detail,
            &crate::style::small(),
            &NSColor::secondaryLabelColor(),
            width,
        );
        views.push(ui::view(&line));
    }
    let refs: Vec<&NSView> = views.iter().map(|v| &**v).collect();
    let column = ui::column(mtm, 2.0, &refs);
    ui::fixed_width(&column, width);
    ui::view(&column)
}
