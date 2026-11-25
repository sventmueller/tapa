//! The browser steps of the self-test (E22), after Dango's browser scenario: `browser:<address>` opens a browser tab
//! in the workspace in front, where `local` or `local/<path>` is a page the self-test serves on 127.0.0.1, `site`
//! opens a workspace with a page among its files first (`site/index.html`, beside `README.html` in the base path),
//! `reload` reloads the browser tab in front and an empty address opens a tab that invites one; `page:<name>` prints
//! the title, address and text of the browser tab in front, and the native page standing in for each tab's web page.
//! Both pages say "JavaScript ran" once their script ran, else "JavaScript is off".

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::OnceLock;

use term_ui::workspace_host::WorkspaceId;

use crate::state::app;

/// The page served on 127.0.0.1 and the file page of the `site` workspace, whose script says that it ran.
const PAGE: &str = "<!doctype html><html><head><meta charset=\"utf-8\"><title>{title}</title>\
    <style>body{font:15px -apple-system;margin:40px;color:#1d1d1f;background:#fff}\
    h1{color:#0a66d8}@media (prefers-color-scheme: dark){body{color:#f5f5f7;background:#1e1e1e}h1{color:#4aa3ff}}\
    </style></head><body><h1>{title}</h1><p>{detail}</p><p id=js>JavaScript is off</p>\
    <script>document.getElementById('js').textContent = 'JavaScript ran'</script></body></html>";

/// Runs `step` if it is one of the browser steps; whether it was.
pub(crate) fn run(step: &str) -> bool {
    let Some((name, argument)) = step.split_once(':') else {
        return false;
    };
    match name {
        "browser" => open(argument),
        "page" => page(argument),
        _ => return false,
    }
    true
}

fn open(argument: &str) {
    let Some(window) = app().front() else {
        return eprintln!("selftest: no window for browser:{argument}");
    };
    if argument == "site" {
        match site() {
            Ok((base, page)) => {
                window.open_workspace(base);
                crate::browser::open(&window, Some(&page.to_string_lossy()));
            }
            Err(error) => eprintln!("selftest: the site's folder: {error}"),
        }
        return;
    }
    if argument == "reload" {
        if let Some(workspace) = window.selected_workspace() {
            term_ui_browser::perform(WorkspaceId::new(workspace.id), objc2::sel!(termBrowserReload:));
        }
        return;
    }
    let url = match argument.strip_prefix("local") {
        Some(path) => match port() {
            Some(port) => Some(format!(
                "http://127.0.0.1:{port}{}",
                if path.is_empty() { "/" } else { path }
            )),
            None => return eprintln!("selftest: the page server did not start"),
        },
        None => (!argument.is_empty()).then(|| argument.to_string()),
    };
    crate::browser::open(&window, url.as_deref());
}

/// Prints what the browser tab in front shows once its page answered.
fn page(name: &str) {
    let Some(workspace) = app().front().and_then(|window| window.selected_workspace()) else {
        return println!("page {name}: no workspace");
    };
    let id = WorkspaceId::new(workspace.id);
    let failures: Vec<String> = term_ui_browser::inspect(id)
        .into_iter()
        .map(|seen| seen.failure.unwrap_or_else(|| "-".into()))
        .collect();
    println!("page {name}: failures {failures:?}");
    let name = name.to_string();
    term_ui_browser::read_front_page(id, move |page| println!("page {name}: {page}"));
}

/// The base path of the `site` workspace with its pages, and the page the step opens.
fn site() -> std::io::Result<(PathBuf, PathBuf)> {
    let dir = std::env::var_os("TAPA_SELFTEST_DIR").map_or_else(|| PathBuf::from("target/selftest"), PathBuf::from);
    let base = dir.join("browser-site");
    std::fs::create_dir_all(base.join("site"))?;
    let base = base.canonicalize()?;
    let page = |title: &str, detail: &str| PAGE.replace("{title}", title).replace("{detail}", detail);
    std::fs::write(
        base.join("site/index.html"),
        page(
            "Project File",
            "A page among the workspace's files. <a href=\"../README.html\">README</a>",
        ),
    )?;
    std::fs::write(base.join("README.html"), page("Readme", "In the base path."))?;
    Ok((base.clone(), base.join("site/index.html")))
}

/// The port of the page server, which starts with the first step that needs it.
fn port() -> Option<u16> {
    static PORT: OnceLock<Option<u16>> = OnceLock::new();
    *PORT.get_or_init(|| {
        let listener = TcpListener::bind("127.0.0.1:0").ok()?;
        let port = listener.local_addr().ok()?.port();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let _ = answer(stream);
            }
        });
        Some(port)
    })
}

fn answer(mut stream: TcpStream) -> std::io::Result<()> {
    let mut request = Vec::new();
    let mut buffer = [0u8; 1024];
    while !request.windows(4).any(|w| w == b"\r\n\r\n") && request.len() < 16 * 1024 {
        let read = stream.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        request.extend_from_slice(&buffer[..read]);
    }
    let line = String::from_utf8_lossy(&request);
    let path = line.split_whitespace().nth(1).unwrap_or("/");
    let (status, body) = if path == "/favicon.ico" {
        ("404 Not Found", String::new())
    } else {
        let detail = format!("Served from 127.0.0.1 for the browser tab at {path}.");
        (
            "200 OK",
            PAGE.replace("{title}", "Browser Selftest").replace("{detail}", &detail),
        )
    };
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\n\
         Connection: close\r\n\r\n{body}",
        body.len()
    )
}
