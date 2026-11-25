//! `/plugins`: the installed plugins to switch, update and remove, the plugins the marketplaces offer to find and
//! install, and the marketplaces themselves. A plugin whose marketplace runs a command to install it shows that
//! command and installs only once the user accepted exactly it.

use std::rc::Rc;

use objc2::rc::Retained;
use objc2_app_kit::NSView;
use splat_i18n::{t, tf};

use super::backend::Backend;
use super::model::{Blocked, MarketChange, Plugin, PluginChange, PluginStep, Plugins, Scope};
use super::permissions::scope_name;
use super::{Open, answer, rows};
use crate::tab::AgentTab;
use crate::ui;
use term_base::explain::Explained;

/// How many offered plugins a search shows.
const SHOWN: usize = 25;

pub(super) fn fill(tab: &AgentTab, open: &Open, backend: &dyn Backend) {
    open.frame.set_rows(&[rows::waiting(tab.mtm)]);
    backend.plugins(Box::new(answer(
        tab.mtm,
        open,
        |tab, open, result: Result<Plugins, Explained>| match result {
            Ok(plugins) => show(tab, open, &Rc::new(plugins), ""),
            Err(why) => rows::failed(tab, open, &why.0),
        },
    )));
}

fn show(tab: &AgentTab, open: &Open, plugins: &Rc<Plugins>, query: &str) {
    let mtm = tab.mtm;
    open.clear_fields();
    let mut views: Vec<Retained<NSView>> = vec![rows::section(mtm, t("Installiert"))];
    if plugins.installed.is_empty() {
        views.push(rows::text(mtm, t("Keine Plugins installiert.")));
    }
    for plugin in &plugins.installed {
        views.push(installed_row(tab, open, plugin));
    }
    views.push(rows::section(mtm, t("Verfügbar")));
    let search = rows::field(mtm, open, "search", t("Plugin suchen"));
    if let Some(field) = (&*search as &objc2::runtime::AnyObject).downcast_ref::<objc2_app_kit::NSTextField>() {
        field.setStringValue(&crate::common::util::ns(query));
    }
    let shown = plugins.clone();
    let find = rows::button(mtm, open, t("Suchen"), move |tab| {
        if let Some(open) = tab.dialog.borrow().clone() {
            let query = open.text("search");
            show(tab, &open, &shown, &query);
        }
    });
    views.push(ui::view(&ui::row(mtm, 8.0, &[&search, &find])));
    let scopes: Vec<(&str, Blocked)> = Scope::ALL.iter().map(|s| (scope_name(*s), None)).collect();
    let scope = rows::choice(mtm, open, "scope", t("Installieren für"), &scopes);
    views.push(rows::labelled(mtm, t("Installieren für"), &scope));
    let found = found(plugins, query);
    if found.is_empty() {
        views.push(rows::text(mtm, t("Kein Plugin gefunden.")));
    }
    for plugin in found {
        views.push(offered_row(tab, open, plugin));
    }
    views.extend(markets(tab, open, plugins));
    open.frame.set_rows(&views);
    open.frame.set_buttons(&[]);
}

/// The offered plugins that match `query`, most installed first; without a query the most installed.
fn found<'a>(plugins: &'a Plugins, query: &str) -> Vec<&'a Plugin> {
    let query = query.to_lowercase();
    let mut found: Vec<&Plugin> = plugins
        .available
        .iter()
        .filter(|p| !plugins.installed.iter().any(|i| i.id == p.id))
        .filter(|p| {
            query.is_empty() || p.name.to_lowercase().contains(&query) || p.description.to_lowercase().contains(&query)
        })
        .collect();
    found.sort_by_key(|p| std::cmp::Reverse(p.installs.unwrap_or(0)));
    found.truncate(SHOWN);
    found
}

fn installed_row(tab: &AgentTab, open: &Open, plugin: &Plugin) -> Retained<NSView> {
    let mtm = tab.mtm;
    let scope = plugin.scope.unwrap_or(Scope::User);
    let mut detail = vec![plugin.marketplace.clone()];
    detail.extend(plugin.version.clone());
    detail.push(scope_name(scope).to_string());
    let text = rows::lines(mtm, rows::BESIDE_TWO - 60.0, &plugin.name, true, &[&detail.join(" · ")]);
    let id = plugin.id.clone();
    let switch = rows::switch(
        mtm,
        open,
        plugin.enabled,
        &tf("{} eingeschaltet", &[&plugin.name]),
        None,
        move |tab, on| {
            let change = if on {
                PluginChange::Enable
            } else {
                PluginChange::Disable
            };
            let id = id.clone();
            super::change(tab, move |backend, done| {
                backend.plugin_change(&id, scope, change, done)
            });
        },
    );
    let id = plugin.id.clone();
    let update = rows::button(mtm, open, t("Neue Version holen"), move |tab| {
        install(tab, &id, scope, true, None)
    });
    let (id, name) = (plugin.id.clone(), plugin.name.clone());
    let remove = rows::button(mtm, open, t("Entfernen"), move |tab| {
        if rows::confirm(tab.mtm, &tf("Plugin „{}“ entfernen?", &[&name]), t("Entfernen")) {
            let id = id.clone();
            super::change(tab, move |backend, done| {
                backend.plugin_change(&id, scope, PluginChange::Uninstall, done);
            });
        }
    });
    ui::view(&ui::row(
        mtm,
        8.0,
        &[&text, &ui::spacer(mtm), &switch, &update, &remove],
    ))
}

fn offered_row(tab: &AgentTab, open: &Open, plugin: &Plugin) -> Retained<NSView> {
    let mtm = tab.mtm;
    let mut detail = plugin.marketplace.clone();
    if let Some(installs) = plugin.installs {
        detail = format!("{detail} · {}", tf("{} Installationen", &[&installs.to_string()]));
    }
    let text = rows::lines(
        mtm,
        rows::BESIDE_ONE,
        &plugin.name,
        true,
        &[&detail, &plugin.description],
    );
    let id = plugin.id.clone();
    let add = rows::button(mtm, open, t("Installieren"), move |tab| {
        let scope = tab
            .dialog
            .borrow()
            .as_ref()
            .and_then(|open| open.chosen("scope"))
            .and_then(|i| Scope::ALL.get(i).copied())
            .unwrap_or(Scope::User);
        install(tab, &id, scope, false, None);
    });
    ui::view(&ui::row(mtm, 8.0, &[&text, &ui::spacer(mtm), &add]))
}

fn markets(tab: &AgentTab, open: &Open, plugins: &Plugins) -> Vec<Retained<NSView>> {
    let mtm = tab.mtm;
    let mut views = vec![rows::section(mtm, t("Marktplätze"))];
    for market in &plugins.marketplaces {
        let text = rows::lines(mtm, rows::BESIDE_TWO, &market.name, true, &[&market.source]);
        let name = market.name.clone();
        let update = rows::button(mtm, open, t("Aktualisieren"), move |tab| {
            let name = name.clone();
            super::change(tab, move |backend, done| {
                backend.market_change(&name, MarketChange::Update, done)
            });
        });
        let name = market.name.clone();
        let remove = rows::button(mtm, open, t("Entfernen"), move |tab| {
            if rows::confirm(tab.mtm, &tf("Marktplatz „{}“ entfernen?", &[&name]), t("Entfernen")) {
                let name = name.clone();
                super::change(tab, move |backend, done| {
                    backend.market_change(&name, MarketChange::Remove, done)
                });
            }
        });
        views.push(ui::view(&ui::row(
            mtm,
            8.0,
            &[&text, &ui::spacer(mtm), &update, &remove],
        )));
    }
    let source = rows::field(mtm, open, "market", t("owner/repo, Adresse oder Ordner"));
    let add = rows::button(mtm, open, t("Hinzufügen"), |tab| {
        let Some(open) = tab.dialog.borrow().clone() else {
            return;
        };
        let source = open.text("market");
        if !source.is_empty() {
            super::change(tab, move |backend, done| {
                backend.market_change(&source, MarketChange::Add, done)
            });
        }
    });
    views.push(ui::view(&ui::row(mtm, 8.0, &[&source, &add])));
    views
}

/// Installs or updates `id`; when its marketplace runs a command for that, shows it and goes on only once the user
/// accepted it.
fn install(tab: &AgentTab, id: &str, scope: Scope, update: bool, accept: Option<String>) {
    let Some(open) = tab.dialog.borrow().clone() else {
        return;
    };
    open.frame.set_notice(Some(t("Wird ausgeführt …")));
    let id = id.to_string();
    let asked = id.clone();
    let done = answer(
        tab.mtm,
        &open,
        move |tab, open, result: Result<PluginStep, Explained>| match result {
            Ok(PluginStep::Done) => tab.refill_dialog(),
            Ok(PluginStep::Confirm { command, sha256 }) => {
                open.frame.set_notice(None);
                let question = tf("„{}“ führt zum Installieren diesen Befehl aus. Zulassen?", &[&asked]);
                if rows::confirm_showing(tab.mtm, &question, &command, t("Befehl zulassen")) {
                    install(tab, &asked, scope, update, Some(sha256));
                }
            }
            Err(why) => open.frame.set_notice(Some(&why.0)),
        },
    );
    super::backend::of(tab).plugin_install(&id, scope, update, accept, Box::new(done));
}
