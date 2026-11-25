//! `/mcp`: the agent's MCP servers with their state, switched on and off, connected again, signed in and out,
//! added and removed. Every agent shows the same rows and form; what its backend cannot do stays greyed with
//! the reason.

use objc2::rc::Retained;
use objc2_app_kit::{NSColor, NSView, NSWorkspace};
use objc2_foundation::NSURL;
use splat_i18n::{t, tf};

use super::backend::{self, Backend};
use super::model::{McpCaps, McpDraft, McpServer, McpState, Scope, Transport};
use super::{Open, answer, rows};
use crate::common::util::ns;
use crate::tab::AgentTab;
use crate::ui;
use term_base::explain::Explained;

pub(super) fn fill(tab: &AgentTab, open: &Open, backend: &dyn Backend) {
    if !super::needs_session(tab, open, backend) {
        return;
    }
    open.frame.set_rows(&[rows::waiting(tab.mtm)]);
    let caps = backend.mcp_caps();
    backend.mcp_servers(Box::new(answer(
        tab.mtm,
        open,
        move |tab, open, result: Result<Vec<McpServer>, Explained>| match result {
            Ok(servers) => show(tab, open, &servers, &caps),
            Err(why) => rows::failed(tab, open, &why.0),
        },
    )));
}

/// What a server's state is called and its dot's colour.
fn state(server: &McpServer) -> (&'static str, objc2::rc::Retained<NSColor>) {
    match server.state {
        McpState::Connected => (t("Verbunden"), NSColor::systemGreenColor()),
        McpState::Pending => (t("Verbindet …"), NSColor::systemOrangeColor()),
        McpState::Failed => (t("Fehlgeschlagen"), NSColor::systemRedColor()),
        McpState::NeedsSignIn => (t("Anmeldung nötig"), NSColor::systemOrangeColor()),
        McpState::Disabled => (t("Ausgeschaltet"), NSColor::tertiaryLabelColor()),
        McpState::Unknown => (t("Unbekannt"), NSColor::tertiaryLabelColor()),
    }
}

/// The line under a server's name: its state, how it is reached, where it lives, its tools or why it failed.
fn detail(server: &McpServer) -> String {
    let mut parts = vec![state(server).0.to_string()];
    parts.extend(server.transport.clone());
    if let Some(scope) = server.scope {
        parts.push(scope_name(scope).to_string());
    }
    if let Some(tools) = server.tools {
        parts.push(tf("{} Werkzeuge", &[&tools.to_string()]));
    }
    let mut line = parts.join(" · ");
    if let Some(why) = &server.detail {
        line = format!("{line}\n{why}");
    }
    line
}

fn scope_name(scope: Scope) -> &'static str {
    match scope {
        Scope::Local => t("Nur dieses Projekt, nur für mich"),
        Scope::Project => t("Projekt (.mcp.json)"),
        Scope::User => t("Alle meine Projekte"),
    }
}

fn show(tab: &AgentTab, open: &Open, servers: &[McpServer], caps: &McpCaps) {
    let mtm = tab.mtm;
    open.clear_fields();
    open.frame.set_notice(caps.live);
    let mut views: Vec<Retained<NSView>> = Vec::new();
    if servers.is_empty() {
        views.push(rows::text(mtm, t("Keine MCP-Server eingerichtet.")));
    }
    for server in servers {
        views.push(server_row(tab, open, server, caps));
    }
    open.frame.set_rows(&views);
    let caps_for_form = caps.clone();
    let add = rows::button(mtm, open, t("Hinzufügen …"), move |tab| {
        if let Some(open) = tab.dialog.borrow().clone() {
            show_form(tab, &open, &caps_for_form);
        }
    });
    rows::block(&add, caps.add);
    let restart = rows::button(mtm, open, t("Sitzung neu starten"), |tab| tab.restart_session());
    rows::block(&restart, caps.restart);
    open.frame.set_buttons(&[ui::view(&restart), ui::view(&add)]);
}

fn server_row(tab: &AgentTab, open: &Open, server: &McpServer, caps: &McpCaps) -> Retained<NSView> {
    let mtm = tab.mtm;
    let (said, colour) = state(server);
    let name = ui::label(mtm, &server.name, &crate::style::bold(13.0), &NSColor::labelColor());
    let named = server.name.clone();
    let switch = rows::switch(
        mtm,
        open,
        server.state != McpState::Disabled,
        &tf("{} eingeschaltet", &[&server.name]),
        caps.toggle,
        move |tab, on| super::change(tab, |backend, done| backend.mcp_toggle(&named, on, done)),
    );
    let dot = rows::dot(mtm, &colour, said);
    let spacer = ui::spacer(mtm);
    let head = ui::row(mtm, 8.0, &[&dot, &name, &spacer, &switch]);
    let detail = ui::wrapping(
        mtm,
        &detail(server),
        &crate::style::small(),
        &NSColor::secondaryLabelColor(),
        500.0,
    );
    let named = server.name.clone();
    let reconnect = rows::button(mtm, open, t("Neu verbinden"), move |tab| {
        super::change(tab, |backend, done| backend.mcp_reconnect(&named, done));
    });
    rows::block(&reconnect, caps.reconnect);
    let mut controls: Vec<Retained<NSView>> = vec![ui::view(&reconnect)];
    if server.remote {
        controls.push(sign_button(tab, open, server, caps));
    }
    if let Some(scope) = server.scope {
        let named = server.name.clone();
        let remove = rows::button(mtm, open, t("Entfernen"), move |tab| {
            if rows::confirm(tab.mtm, &tf("MCP-Server „{}“ entfernen?", &[&named]), t("Entfernen")) {
                super::change(tab, |backend, done| backend.mcp_remove(&named, scope, done));
            }
        });
        let foreign = (!caps.scopes.contains(&scope)).then(|| t("Dieser Agent kann das hier nicht."));
        rows::block(&remove, caps.remove.or(foreign));
        controls.push(ui::view(&remove));
    }
    let buttons: Vec<&NSView> = controls.iter().map(|c| &**c).collect();
    let actions = ui::row(mtm, 6.0, &buttons);
    let column = ui::column(mtm, 4.0, &[&head, &detail, &actions]);
    column.setEdgeInsets(ui::insets(4.0, 0.0, 6.0, 0.0));
    ui::view(&column)
}

/// "Anmelden" for a server that waits for it, else "Abmelden".
fn sign_button(tab: &AgentTab, open: &Open, server: &McpServer, caps: &McpCaps) -> Retained<NSView> {
    let named = server.name.clone();
    let button = if server.state == McpState::NeedsSignIn || server.state == McpState::Failed {
        rows::button(tab.mtm, open, t("Anmelden"), move |tab| sign_in(tab, &named))
    } else {
        rows::button(tab.mtm, open, t("Abmelden"), move |tab| {
            super::change(tab, |backend, done| backend.mcp_sign_out(&named, done));
        })
    };
    rows::block(&button, caps.sign_in);
    ui::view(&button)
}

/// Starts the sign-in to the server `name`: the browser opens at the agent's address, and a field takes the address
/// it lands on when the agent cannot catch it itself.
fn sign_in(tab: &AgentTab, name: &str) {
    let Some(open) = tab.dialog.borrow().clone() else {
        return;
    };
    let named = name.to_string();
    let done = answer(
        tab.mtm,
        &open,
        move |tab, open, result: Result<Option<String>, Explained>| match result {
            Ok(Some(url)) => {
                if let Some(url) = NSURL::URLWithString(&ns(&url)) {
                    NSWorkspace::sharedWorkspace().openURL(&url);
                }
                show_callback(tab, open, &named);
            }
            Ok(None) => tab.refill_dialog(),
            Err(why) => open.frame.set_notice(Some(&why.0)),
        },
    );
    backend::of(tab).mcp_sign_in(name, Box::new(done));
}

fn show_callback(tab: &AgentTab, open: &Open, name: &str) {
    let mtm = tab.mtm;
    open.clear_fields();
    open.frame.set_notice(Some(t("Melde dich im Browser an. Landet der Browser danach auf einer Seite, die nicht lädt, füge ihre Adresse hier ein.")));
    let field = rows::field(mtm, open, "callback", t("Adresse nach der Anmeldung"));
    open.frame.set_rows(&[rows::labelled(mtm, t("Adresse"), &field)]);
    let named = name.to_string();
    let take = rows::button(mtm, open, t("Übernehmen"), move |tab| {
        let Some(open) = tab.dialog.borrow().clone() else {
            return;
        };
        let url = open.text("callback");
        let named = named.clone();
        super::change(tab, move |backend, done| backend.mcp_sign_in_answer(&named, &url, done));
    });
    let done = rows::button(mtm, open, t("Fertig"), |tab| tab.refill_dialog());
    open.frame.set_buttons(&[ui::view(&done), ui::view(&take)]);
}

fn show_form(tab: &AgentTab, open: &Open, caps: &McpCaps) {
    let mtm = tab.mtm;
    open.clear_fields();
    open.frame.set_notice(Some(if caps.scopes.contains(&Scope::Project) {
        t("Ein Server im Projekt landet in .mcp.json, wie eingegeben, und mit Git bei allen, die das Projekt haben.")
    } else {
        t("Ein eigener Server gilt für jede Sitzung dieser Agenten; Umgebung und Header liegen im Schlüsselbund.")
    }));
    let refuse = |allowed: bool| (!allowed).then(|| t("Dieser Agent kann das hier nicht."));
    let transports: Vec<(&str, super::model::Blocked)> = Transport::ALL
        .iter()
        .map(|transport| (transport_name(*transport), refuse(caps.transports.contains(transport))))
        .collect();
    let scopes: Vec<(&str, super::model::Blocked)> = Scope::ALL
        .iter()
        .map(|scope| (scope_name(*scope), refuse(caps.scopes.contains(scope))))
        .collect();
    let scope = rows::choice(mtm, open, "scope", t("Gilt für"), &scopes);
    let first = Scope::ALL.iter().position(|s| caps.scopes.contains(s));
    if let (Some(popup), Some(first)) = (
        (&*scope as &objc2::runtime::AnyObject).downcast_ref::<objc2_app_kit::NSPopUpButton>(),
        first,
    ) {
        popup.selectItemAtIndex(isize::try_from(first).unwrap_or(0));
    }
    open.frame.set_rows(&[
        rows::labelled(mtm, t("Name"), &rows::field(mtm, open, "name", "docs")),
        rows::labelled(
            mtm,
            t("Verbindung"),
            &rows::choice(mtm, open, "transport", t("Verbindung"), &transports),
        ),
        rows::labelled(mtm, t("Befehl"), &rows::field(mtm, open, "command", "npx -y docs-mcp")),
        rows::labelled(
            mtm,
            t("Adresse"),
            &rows::field(mtm, open, "url", "https://example.com/mcp"),
        ),
        rows::labelled(
            mtm,
            t("Umgebung"),
            &rows::field(mtm, open, "env", "NAME=Wert, NAME2=Wert"),
        ),
        rows::labelled(
            mtm,
            t("Header"),
            &rows::field(mtm, open, "headers", "Authorization: Bearer …"),
        ),
        rows::labelled(mtm, t("Gilt für"), &scope),
    ]);
    let cancel = rows::button(mtm, open, t("Abbrechen"), |tab| tab.refill_dialog());
    let add = rows::button(mtm, open, t("Hinzufügen"), |tab| {
        let Some(open) = tab.dialog.borrow().clone() else {
            return;
        };
        match draft(&open) {
            Ok(draft) => super::change(tab, move |backend, done| backend.mcp_add(draft, done)),
            Err(why) => open.frame.set_notice(Some(why)),
        }
    });
    open.frame.set_buttons(&[ui::view(&cancel), ui::view(&add)]);
}

fn transport_name(transport: Transport) -> &'static str {
    match transport {
        Transport::Stdio => t("Programm auf diesem Mac"),
        Transport::Http => "HTTP",
        Transport::Sse => "SSE",
    }
}

/// The form's server, or what is missing.
fn draft(open: &Open) -> Result<McpDraft, &'static str> {
    let name = open.text("name");
    if name.is_empty() {
        return Err(t("Der Server braucht einen Namen."));
    }
    let transport = Transport::ALL[open.chosen("transport").unwrap_or(0).min(2)];
    let command: Vec<String> = open.text("command").split_whitespace().map(str::to_string).collect();
    let url = open.text("url");
    match transport {
        Transport::Stdio if command.is_empty() => return Err(t("Ein Programm braucht einen Befehl.")),
        Transport::Http | Transport::Sse if url.is_empty() => {
            return Err(t("Ein Server im Netz braucht eine Adresse."));
        }
        _ => {}
    }
    Ok(McpDraft {
        name,
        transport,
        command,
        url,
        env: pairs(&open.text("env"), '='),
        headers: pairs(&open.text("headers"), ':'),
        scope: Scope::ALL[open.chosen("scope").unwrap_or(0).min(2)],
    })
}

/// `NAME=value, NAME2=value` (or with `:`) as pairs.
fn pairs(text: &str, separator: char) -> Vec<(String, String)> {
    text.split(',')
        .filter_map(|pair| {
            let (name, value) = pair.split_once(separator)?;
            let name = name.trim();
            (!name.is_empty()).then(|| (name.to_string(), value.trim().to_string()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::pairs;

    #[test]
    fn pairs_read_names_and_values() {
        assert_eq!(
            pairs("A=1, B = two ,=x, C", '='),
            [("A".to_string(), "1".to_string()), ("B".to_string(), "two".to_string())]
        );
        assert_eq!(
            pairs("Authorization: Bearer t", ':'),
            [("Authorization".to_string(), "Bearer t".to_string())]
        );
    }
}
