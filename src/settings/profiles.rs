//! The profiles pane (F161): the list of profiles, adding, renaming, deleting and choosing the default, and each
//! profile's font, colours, command, folder and environment, with what it inherits shown greyed with its origin.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use objc2::rc::Retained;
use objc2::{MainThreadMarker, sel};
use objc2_app_kit::{NSButton, NSStackView, NSView};
use splat_i18n::{t, tf};
use term_config::{Config, Document, EditError, Origin, Part, Profile, number, strings};
use term_ui::kit::{ns, set_action, space};

use super::form::{Fields, Form, Items, Shown, Write};
use super::panes::{format_number, read_number, themes};

/// The profile the pane shows, by name.
pub(super) type Chosen = Rc<RefCell<String>>;

/// The name the pane shows: the chosen one while it exists, else the default profile or the first.
fn shown_name(config: &Config, chosen: &Chosen) -> Option<String> {
    let names: Vec<&str> = config.profiles.iter().map(|p| p.name.as_str()).collect();
    let current = chosen.borrow().clone();
    let name = if names.contains(&current.as_str()) {
        current
    } else if names.contains(&config.default_profile.as_str()) {
        config.default_profile.clone()
    } else {
        (*names.first()?).to_string()
    };
    *chosen.borrow_mut() = name.clone();
    Some(name)
}

fn resolved(config: &Config, chosen: &Chosen) -> Option<Profile> {
    let name = chosen.borrow().clone();
    config.profiles.iter().any(|p| p.name == name).then_some(())?;
    config.profile(&name).ok()
}

fn exists(chosen: &Chosen) -> impl Fn(&Config) -> bool + 'static {
    let chosen = chosen.clone();
    move |config| config.profiles.iter().any(|p| p.name == *chosen.borrow())
}

fn owns(chosen: &Chosen, part: Part) -> impl Fn(&Config) -> bool + 'static {
    let chosen = chosen.clone();
    move |config| config.origin(&chosen.borrow(), part) == Origin::Own
}

/// Where the chosen profile takes `part` from, as a note; empty when it sets it itself.
fn origin_note(config: &Config, chosen: &Chosen, part: Part) -> String {
    let name = chosen.borrow().clone();
    if !config.profiles.iter().any(|p| p.name == name) {
        return String::new();
    }
    match config.origin(&name, part) {
        Origin::Own => String::new(),
        Origin::Profile(from) => tf("Geerbt von „{}“", &[&from]),
        Origin::Settings => t("Geerbt aus den allgemeinen Einstellungen").to_string(),
    }
}

/// What the window says when a change to the profiles was refused.
pub(super) fn explain(error: &EditError) -> String {
    match error {
        EditError::Empty => t("Ein Profil braucht einen Namen.").to_string(),
        EditError::Taken(name) => tf("Es gibt schon ein Profil „{}“.", &[name]),
        EditError::Missing(name) => tf("Es gibt kein Profil „{}“.", &[name]),
        EditError::Layout => {
            t("Die Datei schreibt ihre Profile nicht als [[profile]]-Tabellen; bitte im Texteditor ändern.").to_string()
        }
    }
}

/// A write to the chosen profile.
fn on_profile(
    chosen: &Chosen,
    change: impl Fn(&mut Document, &str, &Shown) -> Result<(), EditError> + 'static,
) -> Write {
    let chosen = chosen.clone();
    Rc::new(move |doc, shown| {
        let name = chosen.borrow().clone();
        change(doc, &name, shown).map_err(|error| explain(&error))?;
        Ok(true)
    })
}

/// The pane.
pub(super) fn pane(fields: &Fields, chosen: &Chosen) -> Retained<NSView> {
    let mut form = Form::new(fields.mtm());
    list_rows(fields, &mut form, chosen);
    font_rows(fields, &mut form, chosen);
    theme_rows(fields, &mut form, chosen);
    session_rows(fields, &mut form, chosen);
    Retained::into_super(form.build())
}

fn button(mtm: MainThreadMarker, fields: &Fields, title: &str, action: objc2::runtime::Sel) -> Retained<NSButton> {
    // SAFETY: A nil target and no action until `set_action` gives it both.
    let button = unsafe { NSButton::buttonWithTitle_target_action(&ns(title), None, None, mtm) };
    set_action(&button, fields.target(), action);
    button
}

fn list_rows(fields: &Fields, form: &mut Form, chosen: &Chosen) {
    let mtm = fields.mtm();
    let items: Items = Box::new(|config| {
        if config.profiles.is_empty() {
            return vec![(t("Kein Profil").to_string(), String::new())];
        }
        config
            .profiles
            .iter()
            .map(|p| {
                let title = if p.name == config.default_profile {
                    tf("{} (Standard)", &[&p.name])
                } else {
                    p.name.clone()
                };
                (title, p.name.clone())
            })
            .collect()
    });
    let (read_chosen, write_chosen) = (chosen.clone(), chosen.clone());
    let (popup, _) = fields.popup(
        items,
        Box::new(move |config| Shown::Text(shown_name(config, &read_chosen).unwrap_or_default())),
        Rc::new(move |_, shown| {
            *write_chosen.borrow_mut() = shown.text().to_string();
            Ok(false)
        }),
    );
    term_ui::kit::set_label(&popup, Some(t("Profil:").trim_end_matches([':', '：', ' ', '\u{a0}'])));
    let add = button(mtm, fields, "+", sel!(termSettingsAddProfile:));
    add.setToolTip(Some(&ns(t("Profil hinzufügen"))));
    term_ui::kit::set_label(&add, Some(t("Profil hinzufügen")));
    let delete = button(mtm, fields, "−", sel!(termSettingsDeleteProfile:));
    delete.setToolTip(Some(&ns(t("Profil löschen"))));
    term_ui::kit::set_label(&delete, Some(t("Profil löschen")));
    let row = NSStackView::stackViewWithViews(
        &objc2_foundation::NSArray::from_retained_slice(&[
            Retained::into_super(Retained::into_super(Retained::into_super(popup))),
            Retained::into_super(Retained::into_super(add)),
            Retained::into_super(Retained::into_super(delete)),
        ]),
        mtm,
    );
    row.setSpacing(space::S);
    form.row(t("Profil:"), &row);
    let read_name = chosen.clone();
    let (name, tag) = fields.text(
        200.0,
        Box::new(move |config| Shown::Text(shown_name(config, &read_name).unwrap_or_default())),
        {
            let chosen = chosen.clone();
            Rc::new(move |doc, shown| {
                let old = chosen.borrow().clone();
                doc.rename_profile(&old, shown.text())
                    .map_err(|error| explain(&error))?;
                *chosen.borrow_mut() = shown.text().trim().to_string();
                Ok(true)
            })
        },
    );
    fields.enable_when(tag, exists(chosen));
    form.row(t("Name:"), &name);
    inherit_rows(fields, form, chosen);
}

fn inherit_rows(fields: &Fields, form: &mut Form, chosen: &Chosen) {
    let parents = chosen.clone();
    let items: Items = Box::new(move |config| {
        let me = parents.borrow().clone();
        std::iter::once((t("Allgemeine Einstellungen").to_string(), String::new()))
            .chain(
                config
                    .profiles
                    .iter()
                    .filter(|p| p.name != me)
                    .map(|p| (p.name.clone(), p.name.clone())),
            )
            .collect()
    });
    let read = chosen.clone();
    let (popup, tag) = fields.popup(
        items,
        Box::new(move |config| {
            let me = read.borrow().clone();
            let parent = config
                .profiles
                .iter()
                .find(|p| p.name == me)
                .and_then(|p| p.inherits.clone());
            Shown::Text(parent.unwrap_or_default())
        }),
        on_profile(chosen, |doc, name, shown| match shown.text() {
            "" => doc.remove_from_profile(name, &["inherits"]),
            parent => doc.set_in_profile(name, &["inherits"], parent),
        }),
    );
    fields.enable_when(tag, exists(chosen));
    form.row(t("Erbt von:"), &popup);
    let read = chosen.clone();
    let default_write = chosen.clone();
    let (check, tag) = fields.check(
        t("Standardprofil für neue Sitzungen"),
        Box::new(move |config| Shown::Bool(config.default_profile == *read.borrow())),
        Rc::new(move |doc, shown| {
            if shown.on() {
                doc.set_default_profile(&default_write.borrow());
            } else {
                doc.remove(&["default_profile"]);
            }
            Ok(true)
        }),
    );
    fields.enable_when(tag, exists(chosen));
    form.row("", &check);
}

fn font_rows(fields: &Fields, form: &mut Form, chosen: &Chosen) {
    form.gap();
    let read = chosen.clone();
    let (own, tag) = fields.check(
        t("Eigene Schrift"),
        Box::new(move |config| Shown::Bool(config.origin(&read.borrow(), Part::Font) == Origin::Own)),
        {
            let chosen = chosen.clone();
            Rc::new(move |doc, shown| {
                let name = chosen.borrow().clone();
                let config = crate::state::app().config();
                let face = config
                    .profile(&name)
                    .ok()
                    .and_then(|p| p.font)
                    .unwrap_or_else(|| config.font.clone());
                let result = if shown.on() {
                    doc.set_in_profile(&name, &["font", "family"], face.family.as_str())
                        .and_then(|()| doc.set_in_profile(&name, &["font", "size"], number(face.size)))
                } else {
                    doc.remove_from_profile(&name, &["font"])
                };
                result.map_err(|error| explain(&error))?;
                Ok(true)
            })
        },
    );
    fields.enable_when(tag, exists(chosen));
    let note_chosen = chosen.clone();
    let note = fields.note(Box::new(move |config| {
        Shown::Text(origin_note(config, &note_chosen, Part::Font))
    }));
    form.row(t("Schrift:"), &own);
    form.under(&note);
    let families: Items = Box::new(|_| super::fonts::monospaced().into_iter().map(|f| (f.clone(), f)).collect());
    let read = chosen.clone();
    let (family, tag) = fields.popup(
        families,
        Box::new(move |config| {
            Shown::Text(
                resolved(config, &read)
                    .and_then(|p| p.font)
                    .map(|f| f.family)
                    .unwrap_or_default(),
            )
        }),
        on_profile(chosen, |doc, name, shown| {
            doc.set_in_profile(name, &["font", "family"], shown.text())
        }),
    );
    fields.enable_when(tag, owns(chosen, Part::Font));
    form.row(t("Familie:"), &family);
    let read = chosen.clone();
    let (size, tag) = fields.text(
        60.0,
        Box::new(move |config| {
            Shown::Text(
                resolved(config, &read)
                    .and_then(|p| p.font)
                    .map(|f| format_number(f.size))
                    .unwrap_or_default(),
            )
        }),
        {
            let chosen = chosen.clone();
            Rc::new(move |doc, shown| {
                let size = read_number(shown.text(), 6.0, 72.0, false)?;
                doc.set_in_profile(&chosen.borrow(), &["font", "size"], number(size))
                    .map_err(|e| explain(&e))?;
                Ok(true)
            })
        },
    );
    fields.enable_when(tag, owns(chosen, Part::Font));
    form.row(t("Größe:"), &size);
}

fn theme_rows(fields: &Fields, form: &mut Form, chosen: &Chosen) {
    form.gap();
    let read = chosen.clone();
    let (own, tag) = fields.check(
        t("Eigene Farben"),
        Box::new(move |config| Shown::Bool(config.origin(&read.borrow(), Part::Theme) == Origin::Own)),
        {
            let chosen = chosen.clone();
            Rc::new(move |doc, shown| {
                let name = chosen.borrow().clone();
                let config = crate::state::app().config();
                let colours = config
                    .profile(&name)
                    .ok()
                    .and_then(|p| p.theme)
                    .unwrap_or_else(|| config.theme.clone());
                let result = if shown.on() {
                    doc.set_in_profile(&name, &["theme", "light"], colours.light.as_str())
                        .and_then(|()| doc.set_in_profile(&name, &["theme", "dark"], colours.dark.as_str()))
                } else {
                    doc.remove_from_profile(&name, &["theme"])
                };
                result.map_err(|error| explain(&error))?;
                Ok(true)
            })
        },
    );
    fields.enable_when(tag, exists(chosen));
    let note_chosen = chosen.clone();
    let note = fields.note(Box::new(move |config| {
        Shown::Text(origin_note(config, &note_chosen, Part::Theme))
    }));
    form.row(t("Farben:"), &own);
    form.under(&note);
    for (title, dark) in [(t("Hell:"), false), (t("Dunkel:"), true)] {
        let read = chosen.clone();
        let key = if dark { "dark" } else { "light" };
        let (popup, tag) = fields.popup(
            themes(dark),
            Box::new(move |config| {
                let colours = resolved(config, &read).and_then(|p| p.theme).unwrap_or_default();
                Shown::Text(if dark { colours.dark } else { colours.light })
            }),
            on_profile(chosen, move |doc, name, shown| {
                doc.set_in_profile(name, &["theme", key], shown.text())
            }),
        );
        fields.enable_when(tag, owns(chosen, Part::Theme));
        form.row(title, &popup);
    }
}

fn session_rows(fields: &Fields, form: &mut Form, chosen: &Chosen) {
    form.gap();
    let read = chosen.clone();
    let (command, tag) = fields.text(
        260.0,
        Box::new(move |config| {
            Shown::Text(
                resolved(config, &read)
                    .and_then(|p| p.command)
                    .map(|c| c.join(" "))
                    .unwrap_or_default(),
            )
        }),
        on_profile(chosen, |doc, name, shown| {
            let words: Vec<&str> = shown.text().split_whitespace().collect();
            if words.is_empty() {
                doc.remove_from_profile(name, &["command"])
            } else {
                doc.set_in_profile(name, &["command"], strings(words))
            }
        }),
    );
    command.setPlaceholderString(Some(&ns(t("Login-Shell"))));
    fields.enable_when(tag, exists(chosen));
    form.row(t("Befehl:"), &command);
    let note_chosen = chosen.clone();
    form.under(&fields.note(Box::new(move |config| {
        Shown::Text(origin_note(config, &note_chosen, Part::Command))
    })));
    let read = chosen.clone();
    let (folder, tag) = fields.text(
        260.0,
        Box::new(move |config| {
            Shown::Text(
                resolved(config, &read)
                    .and_then(|p| p.directory)
                    .map(|d| d.display().to_string())
                    .unwrap_or_default(),
            )
        }),
        on_profile(chosen, |doc, name, shown| match shown.text().trim() {
            "" => doc.remove_from_profile(name, &["directory"]),
            folder => doc.set_in_profile(name, &["directory"], folder),
        }),
    );
    folder.setPlaceholderString(Some(&ns(t("Aktive Sitzung oder Benutzerordner"))));
    fields.enable_when(tag, exists(chosen));
    form.row(t("Ordner:"), &folder);
    let note_chosen = chosen.clone();
    form.under(&fields.note(Box::new(move |config| {
        Shown::Text(origin_note(config, &note_chosen, Part::Directory))
    })));
    env_rows(fields, form, chosen);
}

fn own_env(config: &Config, name: &str) -> BTreeMap<String, String> {
    config
        .profiles
        .iter()
        .find(|p| p.name == name)
        .map(|p| p.env.clone())
        .unwrap_or_default()
}

fn env_text(env: &BTreeMap<String, String>) -> String {
    env.iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn env_rows(fields: &Fields, form: &mut Form, chosen: &Chosen) {
    let read = chosen.clone();
    let (env, tag) = fields.text(
        260.0,
        Box::new(move |config| Shown::Text(env_text(&own_env(config, &read.borrow())))),
        {
            let chosen = chosen.clone();
            Rc::new(move |doc, shown| {
                let mut table = toml_edit::InlineTable::new();
                for entry in shown.text().split_whitespace() {
                    let Some((key, value)) = entry.split_once('=').filter(|(key, _)| !key.is_empty()) else {
                        return Err(tf("„{}“ ist keine Variable wie NAME=Wert.", &[entry]));
                    };
                    table.insert(key, value.into());
                }
                let name = chosen.borrow().clone();
                let result = if table.is_empty() {
                    doc.remove_from_profile(&name, &["env"])
                } else {
                    doc.set_in_profile(&name, &["env"], toml_edit::Value::InlineTable(table))
                };
                result.map_err(|error| explain(&error))?;
                Ok(true)
            })
        },
    );
    env.setPlaceholderString(Some(&ns("NAME=Wert")));
    fields.enable_when(tag, exists(chosen));
    form.row(t("Umgebung:"), &env);
    let read = chosen.clone();
    let note = fields.note(Box::new(move |config| {
        let name = read.borrow().clone();
        let own = own_env(config, &name);
        let inherited: BTreeMap<String, String> = config
            .profile(&name)
            .map(|p| p.env)
            .unwrap_or_default()
            .into_iter()
            .filter(|(key, _)| !own.contains_key(key))
            .collect();
        Shown::Text(if inherited.is_empty() {
            String::new()
        } else {
            tf("Geerbt: {}", &[&env_text(&inherited)])
        })
    }));
    form.under(&note);
}

/// Asks before deleting the profile `name`; whether the user agreed.
pub(super) fn confirm_delete(mtm: MainThreadMarker, name: &str) -> bool {
    crate::quit::confirm(
        mtm,
        &tf("Profil „{}“ löschen?", &[name]),
        t(
            "Profile, die von ihm erben, erben danach von seinem Elternprofil. Der Eintrag verschwindet aus der Einstellungsdatei.",
        ),
        t("Löschen"),
    )
}
