//! The panes whose controls each edit one setting: window, font, colours, cursor, terminal and input.

use std::rc::Rc;

use objc2::rc::Retained;
use objc2::sel;
use objc2_app_kit::{NSButton, NSStackView, NSView};
use objc2_foundation::NSArray;
use splat_i18n::{t, tf};
use term_config::{Config, CursorStyle, OptionAsMeta, QuickPosition, number, strings};
use term_ui::kit::{label, ns, set_action, space};

use super::form::{Fields, Form, Items, Read, Shown, Write};

/// A setting's place in the file.
type Path = &'static [&'static str];
/// A checkbox row: its label, the checkbox's title, how to read it and where it is written.
type CheckRow = (&'static str, &'static str, fn(&Config) -> bool, Path);

pub(super) fn flag(get: fn(&Config) -> bool, path: Path) -> (Read, Write) {
    (
        Box::new(move |config| Shown::Bool(get(config))),
        Rc::new(move |doc, shown| {
            doc.set(path, shown.on());
            Ok(true)
        }),
    )
}

pub(super) fn word(get: fn(&Config) -> String, path: Path) -> (Read, Write) {
    (
        Box::new(move |config| Shown::Text(get(config))),
        Rc::new(move |doc, shown| {
            doc.set(path, shown.text());
            Ok(true)
        }),
    )
}

/// A number from `low` to `high`, whole when `whole`.
pub(super) fn amount(get: fn(&Config) -> f64, path: Path, low: f64, high: f64, whole: bool) -> (Read, Write) {
    (
        Box::new(move |config| Shown::Text(format_number(get(config)))),
        Rc::new(move |doc, shown| {
            let value = read_number(shown.text(), low, high, whole)?;
            doc.set(path, number(value));
            Ok(true)
        }),
    )
}

/// `value` with the decimal separator of the user's region.
pub(super) fn format_number(value: f64) -> String {
    if value.fract() == 0.0 {
        format!("{value:.0}")
    } else {
        let separator = objc2_foundation::NSLocale::currentLocale()
            .decimalSeparator()
            .to_string();
        format!("{value}").replace('.', &separator)
    }
}

pub(super) fn read_number(text: &str, low: f64, high: f64, whole: bool) -> Result<f64, String> {
    let text = text.trim().replace(',', ".");
    match text.parse::<f64>() {
        Ok(value) if (low..=high).contains(&value) && (!whole || value.fract() == 0.0) => Ok(value),
        _ => Err(tf(
            "„{}“ ist keine Zahl von {} bis {}.",
            &[&text, &format_number(low), &format_number(high)],
        )),
    }
}

/// A pop-up of fixed choices, title and the value the file writes.
pub(super) fn fixed(choices: Vec<(&'static str, &'static str)>) -> Items {
    Box::new(move |_| {
        choices
            .iter()
            .map(|(title, value)| ((*title).to_string(), (*value).to_string()))
            .collect()
    })
}

pub(super) fn text_row(fields: &Fields, form: &mut Form, title: &str, width: f64, (read, write): (Read, Write)) {
    let (field, _) = fields.text(width, read, write);
    form.row(title, &field);
}

pub(super) fn check_row(fields: &Fields, form: &mut Form, title: &str, label: &str, (read, write): (Read, Write)) {
    let (button, _) = fields.check(label, read, write);
    form.row(title, &button);
}

pub(super) fn popup_row(fields: &Fields, form: &mut Form, title: &str, items: Items, (read, write): (Read, Write)) {
    let (button, _) = fields.popup(items, read, write);
    form.row(title, &button);
}

/// Windows, tabs and panes.
pub(super) fn window(fields: &Fields) -> Retained<NSView> {
    let mut form = Form::new(fields.mtm());
    popup_row(
        fields,
        &mut form,
        t("Erscheinungsbild:"),
        fixed(vec![
            (t("Wie das System"), "system"),
            (t("Hell"), "light"),
            (t("Dunkel"), "dark"),
        ]),
        word(|c| c.window.appearance.id().to_string(), &["window", "appearance"]),
    );
    form.gap();
    font_sizes(fields, &mut form);
    form.gap();
    check_row(
        fields,
        &mut form,
        t("Fenster:"),
        t("Seitenleiste in neuen Fenstern zeigen"),
        flag(|c| c.window.sidebar, &["window", "sidebar"]),
    );
    form.hint(t(
        "Die Seitenleiste listet die Arbeitsbereiche; ihre Taste in der Werkzeugleiste blendet sie je Fenster aus.",
    ));
    check_row(
        fields,
        &mut form,
        "",
        t("Vor dem Schließen laufender Programme fragen"),
        flag(|c| c.window.confirm_close, &["window", "confirm_close"]),
    );
    form.gap();
    size_row(fields, &mut form);
    form.gap();
    let inherit: [CheckRow; 2] = [
        (
            t("Ordner übernehmen:"),
            t("Neues Fenster"),
            |c| c.window.inherit_directory.window,
            &["window", "inherit_directory", "window"],
        ),
        (
            "",
            t("Neuer Bereich"),
            |c| c.window.inherit_directory.split,
            &["window", "inherit_directory", "split"],
        ),
    ];
    for (title, check, get, path) in inherit {
        check_row(fields, &mut form, title, check, flag(get, path));
    }
    form.hint(t("Ein neues Fenster startet im Ordner des Arbeitsbereichs, ein neuer Bereich in dem der aktiven Sitzung. Neue Tabs starten immer im Ordner ihres Arbeitsbereichs."));
    quick_and_restore(fields, &mut form);
    form.gap();
    let engines = fixed(
        term_config::SearchEngine::ALL
            .iter()
            .map(|engine| (engine.name(), engine.id()))
            .collect(),
    );
    popup_row(
        fields,
        &mut form,
        t("Suchmaschine:"),
        engines,
        word(
            |c: &Config| c.browser.search_engine.id().to_string(),
            &["browser", "search_engine"],
        ),
    );
    form.gap();
    // SAFETY: A nil target and no action until `set_action` gives it both.
    let open =
        unsafe { NSButton::buttonWithTitle_target_action(&ns(t("Im Texteditor öffnen")), None, None, fields.mtm()) };
    set_action(&open, fields.target(), sel!(termSettingsOpenFile:));
    form.row(t("Einstellungsdatei:"), &open);
    Retained::into_super(form.build())
}

/// The text sizes in one place, as Dango has them: the interface's, the editors' code, the terminal's and the agent
/// tabs'.
fn font_sizes(fields: &Fields, form: &mut Form) {
    let sizes: [(&str, &str, (Read, Write)); 4] = [
        (
            t("Schriftgröße:"),
            t("Oberfläche"),
            amount(
                |c| c.window.interface_font_size,
                &["window", "interface_font_size"],
                term_ui::kit::ui_size::MIN,
                term_ui::kit::ui_size::MAX,
                false,
            ),
        ),
        (
            "",
            t("Editor"),
            amount(|c| c.editor.font_size, &["editor", "font_size"], 8.0, 36.0, false),
        ),
        (
            "",
            t("Terminal"),
            amount(|c| c.font.size, &["font", "size"], 6.0, 72.0, false),
        ),
        (
            "",
            t("Agenten"),
            amount(
                |c| c.agent.font_size,
                &["agent", "font_size"],
                term_ui_agent::CHAT_FONT_MIN,
                term_ui_agent::CHAT_FONT_MAX,
                false,
            ),
        ),
    ];
    for (title, caption, (read, write)) in sizes {
        let (field, _) = fields.text(60.0, read, write);
        let caption = label(fields.mtm(), caption);
        term_ui::kit::set_title_element(&field, &caption);
        let views: [Retained<NSView>; 2] = [
            Retained::into_super(Retained::into_super(field)),
            Retained::into_super(Retained::into_super(caption)),
        ];
        let row = NSStackView::stackViewWithViews(&NSArray::from_retained_slice(&views), fields.mtm());
        row.setSpacing(space::S);
        row.setAlignment(objc2_app_kit::NSLayoutAttribute::FirstBaseline);
        form.row(title, &row);
    }
    form.hint(t(
        "Der Oberfläche folgen Seitenleiste, Tabs, Panels, Fenstertitel und die Leisten des Editors.",
    ));
}

/// The window's size in columns and rows, in one row.
fn size_row(fields: &Fields, form: &mut Form) {
    let mtm = fields.mtm();
    let (read, write) = amount(
        |c| f64::from(c.window.columns),
        &["window", "columns"],
        20.0,
        1000.0,
        true,
    );
    let (columns, _) = fields.text(60.0, read, write);
    let (read, write) = amount(|c| f64::from(c.window.rows), &["window", "rows"], 5.0, 500.0, true);
    let (rows, _) = fields.text(60.0, read, write);
    let (columns_caption, rows_caption) = (label(mtm, t("Spalten")), label(mtm, t("Zeilen")));
    term_ui::kit::set_title_element(&columns, &columns_caption);
    term_ui::kit::set_title_element(&rows, &rows_caption);
    let views: [Retained<NSView>; 4] = [
        Retained::into_super(Retained::into_super(columns)),
        Retained::into_super(Retained::into_super(columns_caption)),
        Retained::into_super(Retained::into_super(rows)),
        Retained::into_super(Retained::into_super(rows_caption)),
    ];
    let row = NSStackView::stackViewWithViews(&NSArray::from_retained_slice(&views), mtm);
    row.setSpacing(space::S);
    row.setAlignment(objc2_app_kit::NSLayoutAttribute::FirstBaseline);
    form.row(t("Fenstergröße:"), &row);
}

/// The quick terminal (F45) and what comes back after quitting (F71), in the window pane.
fn quick_and_restore(fields: &Fields, form: &mut Form) {
    form.gap();
    check_row(
        fields,
        form,
        t("Schnelles Terminal:"),
        t("Ausblenden, wenn es den Fokus verliert"),
        flag(|c| c.quick_terminal.autohide, &["quick_terminal", "autohide"]),
    );
    let edges = fixed(vec![
        (t("Oben"), "top"),
        (t("Unten"), "bottom"),
        (t("Links"), "left"),
        (t("Rechts"), "right"),
    ]);
    let edge = |c: &Config| {
        match c.quick_terminal.position {
            QuickPosition::Top => "top",
            QuickPosition::Bottom => "bottom",
            QuickPosition::Left => "left",
            QuickPosition::Right => "right",
        }
        .to_string()
    };
    popup_row(
        fields,
        form,
        t("Rand:"),
        edges,
        word(edge, &["quick_terminal", "position"]),
    );
    text_row(
        fields,
        form,
        t("Tiefe:"),
        60.0,
        amount(|c| c.quick_terminal.size, &["quick_terminal", "size"], 0.1, 1.0, false),
    );
    text_row(
        fields,
        form,
        t("Breite:"),
        60.0,
        amount(|c| c.quick_terminal.span, &["quick_terminal", "span"], 0.2, 1.0, false),
    );
    form.hint(t("Anteil am Bildschirm, von 0,1 bis 1"));
    form.gap();
    check_row(
        fields,
        form,
        t("Wiederherstellen:"),
        t("Fenster, Tabs und Bereiche nach dem Neustart"),
        flag(|c| c.restore.enabled, &["restore", "enabled"]),
    );
    check_row(
        fields,
        form,
        "",
        t("Auch den Verlauf, verschlüsselt"),
        flag(|c| c.restore.scrollback, &["restore", "scrollback"]),
    );
    #[expect(clippy::cast_precision_loss, reason = "a line count far below 2^52")]
    let lines = |c: &Config| c.restore.scrollback_lines as f64;
    text_row(
        fields,
        form,
        t("Zeilen je Sitzung:"),
        70.0,
        amount(lines, &["restore", "scrollback_lines"], 0.0, 10_000.0, true),
    );
}

/// The face of the grid.
pub(super) fn font(fields: &Fields) -> Retained<NSView> {
    let mut form = Form::new(fields.mtm());
    let families: Items = Box::new(|_| {
        super::fonts::monospaced()
            .into_iter()
            .map(|family| (family.clone(), family))
            .collect()
    });
    popup_row(
        fields,
        &mut form,
        t("Schrift:"),
        families,
        word(|c| c.font.family.clone(), &["font", "family"]),
    );
    text_row(
        fields,
        &mut form,
        t("Zeilenhöhe:"),
        60.0,
        amount(|c| c.font.line_height, &["font", "line_height"], 0.5, 3.0, false),
    );
    check_row(
        fields,
        &mut form,
        "",
        t("Ligaturen"),
        flag(|c| c.font.ligatures, &["font", "ligatures"]),
    );
    let features: Write = Rc::new(|doc, shown| {
        let list: Vec<&str> = shown.text().split_whitespace().collect();
        if list.is_empty() {
            doc.remove(&["font", "features"]);
        } else {
            doc.set(&["font", "features"], strings(list));
        }
        Ok(true)
    });
    form.gap();
    let (field, _) = fields.text(260.0, Box::new(|c| Shown::Text(c.font.features.join(" "))), features);
    field.setPlaceholderString(Some(&ns("ss01 -calt cv05=2")));
    form.row(t("OpenType-Features:"), &field);
    let (field, _) = fields.text(
        260.0,
        Box::new(|c| Shown::Text(variations_text(c))),
        Rc::new(write_variations),
    );
    field.setPlaceholderString(Some(&ns("wght=450")));
    form.row(t("Achsen:"), &field);
    form.hint(t("Mehrere Einträge durch Leerzeichen trennen."));
    Retained::into_super(form.build())
}

fn variations_text(config: &Config) -> String {
    config
        .font
        .variations
        .iter()
        .map(|(tag, value)| format!("{tag}={}", format_number(*value)))
        .collect::<Vec<_>>()
        .join(" ")
}

fn write_variations(doc: &mut term_config::Document, shown: &Shown) -> Result<bool, String> {
    let mut table = toml_edit::InlineTable::new();
    for entry in shown.text().split_whitespace() {
        let parsed = entry
            .split_once('=')
            .and_then(|(tag, value)| Some((tag.trim(), value.trim().replace(',', ".").parse::<f64>().ok()?)));
        let Some((tag, value)) = parsed.filter(|(tag, _)| tag.len() == 4) else {
            return Err(tf("„{}“ ist keine Achse wie wght=450.", &[entry]));
        };
        table.insert(tag, number(value));
    }
    if table.is_empty() {
        doc.remove(&["font", "variations"]);
    } else {
        doc.set(&["font", "variations"], toml_edit::Value::InlineTable(table));
    }
    Ok(true)
}

/// The bundled and the user's themes of one appearance, the other's after them.
pub(super) fn themes(dark: bool) -> Items {
    Box::new(move |_| {
        let themes = crate::state::app().themes.borrow().clone();
        let (mut fitting, mut others): (Vec<String>, Vec<String>) = (Vec::new(), Vec::new());
        for name in themes.names() {
            if themes.get(name, dark, false).is_dark() == dark {
                fitting.push(name.to_string());
            } else {
                others.push(name.to_string());
            }
        }
        fitting
            .into_iter()
            .chain(others)
            .map(|name| (name.clone(), name))
            .collect()
    })
}

/// The colours.
pub(super) fn theme(fields: &Fields) -> Retained<NSView> {
    let mut form = Form::new(fields.mtm());
    popup_row(
        fields,
        &mut form,
        t("Hell:"),
        themes(false),
        word(|c| c.theme.light.clone(), &["theme", "light"]),
    );
    popup_row(
        fields,
        &mut form,
        t("Dunkel:"),
        themes(true),
        word(|c| c.theme.dark.clone(), &["theme", "dark"]),
    );
    text_row(
        fields,
        &mut form,
        t("Mindestkontrast:"),
        60.0,
        amount(
            |c| c.theme.minimum_contrast,
            &["theme", "minimum_contrast"],
            1.0,
            21.0,
            false,
        ),
    );
    form.hint(t(
        "1 lässt die Farben des Themes, wie sie sind; 4,5 ist die Grenze für lesbaren Text.",
    ));
    Retained::into_super(form.build())
}

/// The cursor.
pub(super) fn cursor(fields: &Fields) -> Retained<NSView> {
    let mut form = Form::new(fields.mtm());
    let styles = fixed(vec![
        (t("Block"), "block"),
        (t("Strich"), "bar"),
        (t("Unterstrich"), "underline"),
    ]);
    let style = |c: &Config| {
        match c.cursor.style {
            CursorStyle::Block => "block",
            CursorStyle::Bar => "bar",
            CursorStyle::Underline => "underline",
        }
        .to_string()
    };
    popup_row(fields, &mut form, t("Form:"), styles, word(style, &["cursor", "style"]));
    check_row(
        fields,
        &mut form,
        "",
        t("Blinken"),
        flag(|c| c.cursor.blink, &["cursor", "blink"]),
    );
    check_row(
        fields,
        &mut form,
        "",
        t("Zeile des Cursors hervorheben"),
        flag(|c| c.cursor.guide, &["cursor", "guide"]),
    );
    Retained::into_super(form.build())
}

/// How the terminal treats programs and keys.
pub(super) fn terminal(fields: &Fields) -> Retained<NSView> {
    let mut form = Form::new(fields.mtm());
    #[expect(clippy::cast_precision_loss, reason = "a line count far below 2^52")]
    let scrollback = |c: &Config| c.terminal.scrollback as f64;
    text_row(
        fields,
        &mut form,
        t("Verlauf in Zeilen:"),
        90.0,
        amount(scrollback, &["terminal", "scrollback"], 0.0, 10_000_000.0, true),
    );
    let metas = fixed(vec![
        (t("Keine"), "none"),
        (t("Linke"), "left"),
        (t("Rechte"), "right"),
        (t("Beide"), "both"),
    ]);
    let meta = |c: &Config| {
        match c.terminal.option_as_meta {
            OptionAsMeta::None => "none",
            OptionAsMeta::Left => "left",
            OptionAsMeta::Right => "right",
            OptionAsMeta::Both => "both",
        }
        .to_string()
    };
    popup_row(
        fields,
        &mut form,
        t("⌥ als Meta:"),
        metas,
        word(meta, &["terminal", "option_as_meta"]),
    );
    form.hint(t("Die gewählte Wahltaste sendet Escape-Folgen statt Sonderzeichen."));
    form.gap();
    check_row(
        fields,
        &mut form,
        t("Zwischenablage:"),
        t("Auswahl sofort kopieren"),
        flag(|c| c.terminal.copy_on_select, &["terminal", "copy_on_select"]),
    );
    check_row(
        fields,
        &mut form,
        "",
        t("Programme dürfen die Zwischenablage lesen (OSC 52)"),
        flag(
            |c| c.terminal.allow_clipboard_read,
            &["terminal", "allow_clipboard_read"],
        ),
    );
    check_row(
        fields,
        &mut form,
        "",
        t("Mehrzeiliges Einsetzen vorher zeigen"),
        flag(|c| c.terminal.paste_protection, &["terminal", "paste_protection"]),
    );
    form.gap();
    check_row(
        fields,
        &mut form,
        t("Tastatur:"),
        t("Kitty-Tastaturprotokoll"),
        flag(|c| c.terminal.kitty_keyboard, &["terminal", "kitty_keyboard"]),
    );
    form.hint(t(
        "Gibt Programmen wie Neovim jede Taste mit ihren Sondertasten weiter.",
    ));
    form.gap();
    integration_rows(fields, &mut form);
    Retained::into_super(form.build())
}

/// The rows of the shell integration and what builds on it.
fn integration_rows(fields: &Fields, form: &mut Form) {
    check_row(
        fields,
        form,
        t("Shell:"),
        t("Shell-Integration"),
        flag(|c| c.terminal.shell_integration, &["terminal", "shell_integration"]),
    );
    form.hint(t(
        "Ermöglicht Prompt-Sprünge, Befehlsblöcke und den Ordner neuer Sitzungen.",
    ));
    check_row(
        fields,
        form,
        "",
        t("Shell-Integration auch über SSH"),
        flag(|c| c.terminal.ssh_integration, &["terminal", "ssh_integration"]),
    );
    check_row(
        fields,
        form,
        "",
        t("Befehle als Blöcke markieren"),
        flag(|c| c.terminal.blocks, &["terminal", "blocks"]),
    );
}

/// The input field and suggestions.
pub(super) fn input(fields: &Fields) -> Retained<NSView> {
    let mut form = Form::new(fields.mtm());
    check_row(
        fields,
        &mut form,
        t("Neue Sitzungen:"),
        t("Mit Eingabefeld"),
        flag(|c| c.input.field, &["input", "field"]),
    );
    check_row(
        fields,
        &mut form,
        "",
        t("Vim-Tasten im Eingabefeld"),
        flag(|c| c.input.vim, &["input", "vim"]),
    );
    check_row(
        fields,
        &mut form,
        "",
        t("Vorschläge beim Tippen"),
        flag(|c| c.input.suggestions, &["input", "suggestions"]),
    );
    check_row(
        fields,
        &mut form,
        "",
        t("Befehle einfärben"),
        flag(|c| c.input.highlighting, &["input", "highlighting"]),
    );
    check_row(
        fields,
        &mut form,
        "",
        t("Unbekannte Befehle unterstreichen"),
        flag(|c| c.input.underline_errors, &["input", "underline_errors"]),
    );
    Retained::into_super(form.build())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_are_read_in_their_range_and_with_either_decimal_mark() {
        assert_eq!(read_number(" 13,5 ", 6.0, 72.0, false), Ok(13.5));
        assert!(read_number("13.5", 6.0, 72.0, true).is_err());
        assert!(read_number("100", 6.0, 72.0, false).is_err());
        assert!(read_number("big", 6.0, 72.0, false).is_err());
        assert_eq!(format_number(14.0), "14");
        let separator = objc2_foundation::NSLocale::currentLocale()
            .decimalSeparator()
            .to_string();
        assert_eq!(format_number(1.25), format!("1{separator}25"));
    }
}
