//! The editor pane (E22): Vim's keys, whitespace and wrapping in the editor tabs, and which
//! files the file list of a workspace's sidebar entry shows.

use objc2::rc::Retained;
use objc2_app_kit::NSView;
use splat_i18n::t;

use super::form::{Fields, Form};
use super::panes::{check_row, flag};

pub(super) fn pane(fields: &Fields) -> Retained<NSView> {
    let mut form = Form::new(fields.mtm());
    check_row(
        fields,
        &mut form,
        t("Tastatur:"),
        t("Vim-Tasten"),
        flag(|c| c.editor.vim, &["editor", "vim"]),
    );
    check_row(
        fields,
        &mut form,
        "",
        t("Vim-Register ist die Zwischenablage"),
        flag(|c| c.editor.vim_clipboard, &["editor", "vim_clipboard"]),
    );
    form.gap();
    check_row(
        fields,
        &mut form,
        t("Darstellung:"),
        t("Leerzeichen und Tabulatoren zeigen"),
        flag(|c| c.editor.show_whitespace, &["editor", "show_whitespace"]),
    );
    check_row(
        fields,
        &mut form,
        "",
        t("Lange Zeilen umbrechen"),
        flag(|c| c.editor.wrap, &["editor", "wrap"]),
    );
    form.hint(t("Markdown bricht immer um."));
    form.gap();
    check_row(
        fields,
        &mut form,
        t("Dateiliste:"),
        t("Ignorierte und versteckte Dateien zeigen"),
        flag(|c| c.files.show_ignored, &["files", "show_ignored"]),
    );
    form.gap();
    form.wide(&super::form::secondary(
        fields.mtm(),
        t("Der Editor startet keine Formatierer und keinen Sprachserver; Sicherungskopien ungesicherter Texte sind verschlüsselt."),
    ));
    Retained::into_super(form.build())
}
