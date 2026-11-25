//! The preview steps of the self-test (E22): `preview:demo` opens a workspace with a Markdown document holding a
//! formula, a mermaid diagram and a picture, and `long.md` of 60 sections beside it, opens the first in an editor and
//! shows its preview beside it; `preview:<path>` does the same for a Markdown file, or opens a picture, PDF or Quick
//! Look file in a tab that shows it, the path taken from the workspace's base unless absolute; `previewtext:<name>`
//! prints what the preview of the editor in front shows; `previewscroll:<line>` scrolls that editor to line `line` and
//! prints the line the preview then shows at its top; `decoder:<name>` decodes the demo picture through the decoding
//! service and prints its size, the service's pid beside the app's, and whether its sandbox refused a file and the
//! network.

use std::path::{Path, PathBuf};
use std::time::Duration;

use term_ui::workspace_host::{Workspace, WorkspaceId};

use crate::state::app;

/// The demo document: a formula, a diagram and a picture beside it.
const DEMO: &str = "# Preview\n\nA formula inline, $e^{i\\pi} + 1 = 0$, and as a block:\n\n$$\n\\int_0^1 x^2 \\, dx = \\frac{1}{3}\n$$\n\n```mermaid\nflowchart LR\n    A[Editor] --> B[Preview]\n    B --> C[Decoder.xpc]\n```\n\n![A gradient](picture.png)\n\n- [x] decoded in the sandbox\n- [ ] read in the app\n";

/// Runs `step` if it is one of the preview steps; whether it was.
pub(super) fn run(step: &str) -> bool {
    let Some((name, argument)) = step.split_once(':') else {
        return false;
    };
    match name {
        "preview" => open(argument),
        "previewtext" => text(argument),
        "decoder" => decoder(argument),
        "previewscroll" => scroll(argument),
        _ => return false,
    }
    true
}

fn open(argument: &str) {
    let Some(window) = app().front() else {
        return eprintln!("selftest: no window for preview:{argument}");
    };
    let path = if argument == "demo" {
        match demo() {
            Ok(base) => {
                window.open_workspace(base.clone());
                base.join("demo.md")
            }
            Err(error) => return eprintln!("selftest: the demo folder: {error}"),
        }
    } else {
        let Some(workspace) = window.selected_workspace() else {
            return eprintln!("selftest: no workspace for preview:{argument}");
        };
        workspace.base.join(argument)
    };
    let Some(workspace) = window.selected_workspace() else {
        return eprintln!("selftest: no workspace for preview:{argument}");
    };
    let id = WorkspaceId::new(workspace.id);
    if crate::preview_tab::open_document(id, &path) {
        return;
    }
    term_ui_editor::open_tab(&Workspace::new(id), &path, false);
    show_beside_when_read(id, path, 40);
}

/// Turns the preview on once the editor of `path` read its file, looking `tries` more times.
fn show_beside_when_read(workspace: WorkspaceId, path: PathBuf, tries: u32) {
    let editor = term_ui_editor::focused_editor(&Workspace::new(workspace)).filter(|&editor| {
        term_ui_editor::text_of(editor)
            .is_some_and(|(shown, text)| shown.as_deref() == Some(path.as_path()) && !text.is_empty())
    });
    match editor {
        Some(editor) => {
            if !crate::preview_tab::shown(editor) {
                crate::preview_tab::toggle(workspace, editor);
            }
        }
        None if tries > 0 => term_base::main_after(Duration::from_millis(50), move || {
            show_beside_when_read(workspace, path, tries - 1);
        }),
        None => eprintln!("selftest: no editor read {}", path.display()),
    }
}

fn text(name: &str) {
    let editor = app()
        .front()
        .and_then(|window| window.selected_workspace())
        .and_then(|workspace| term_ui_editor::focused_editor(&Workspace::new(WorkspaceId::new(workspace.id))));
    let Some((text, attachments)) = editor.and_then(term_ui_preview::selftest::text) else {
        return println!("previewtext {name}: no preview");
    };
    let top = editor.and_then(term_ui_preview::selftest::top_line).unwrap_or_default();
    println!("previewtext {name}: attachments {attachments} top {top:?} text {text:?}");
}

/// Scrolls the editor in front so that line `line` of its text is at the top and prints the line the preview beside
/// it then shows at its top.
fn scroll(line: &str) {
    let Ok(line) = line.parse::<usize>() else {
        return eprintln!("selftest: previewscroll needs a line number, not {line}");
    };
    let editor = app()
        .front()
        .and_then(|window| window.selected_workspace())
        .and_then(|workspace| term_ui_editor::focused_editor(&Workspace::new(WorkspaceId::new(workspace.id))));
    let Some(editor) = editor else {
        return println!("previewscroll {line}: no editor");
    };
    let Some((_, text)) = term_ui_editor::text_of(editor) else {
        return;
    };
    let byte: usize = text
        .split_inclusive('\n')
        .take(line.saturating_sub(1))
        .map(str::len)
        .sum();
    let (Some(view), Some(y)) = (
        term_ui_editor::text_view(editor),
        term_ui_editor::byte_y(editor, byte, false),
    ) else {
        return println!("previewscroll {line}: no place");
    };
    view.scrollPoint(objc2_foundation::NSPoint::new(0.0, y));
    let shown = text.lines().nth(line.saturating_sub(1)).unwrap_or_default();
    let top = term_ui_preview::selftest::top_line(editor);
    println!("previewscroll {line}: editor top {shown:?} preview top {top:?}");
}

fn decoder(name: &str) {
    let picture = match demo() {
        Ok(base) => base.join("picture.png"),
        Err(error) => return println!("decoder {name}: the demo folder: {error}"),
    };
    let size = term_ui_preview::selftest::decode_file(&picture);
    let app_pid = std::process::id();
    let services = term_ui_preview::selftest::service_pids();
    let apart = !services.is_empty() && services.iter().all(|&pid| u32::try_from(pid).ok() != Some(app_pid));
    let sandboxed = term_ui_preview::selftest::sandboxed(&picture.to_string_lossy());
    println!(
        "decoder {name}: decoded {size:?} through the service, service pids {services:?}, app pid {app_pid}, \
         apart {apart}, sandbox refused file and network {sandboxed:?}"
    );
}

/// Writes the demo folder with its document and picture; its path.
fn demo() -> std::io::Result<PathBuf> {
    let dir = std::env::var_os("TAPA_SELFTEST_DIR").map_or_else(|| PathBuf::from("target/selftest"), PathBuf::from);
    let base = dir.join("preview-demo");
    std::fs::create_dir_all(&base)?;
    let base = base.canonicalize()?;
    std::fs::write(base.join("demo.md"), DEMO)?;
    let long: String = (1..=60)
        .map(|n| {
            format!("## Section {n}\n\nThe text of section {n}, long enough to wrap in a narrow preview beside it.\n\n")
        })
        .collect();
    std::fs::write(base.join("long.md"), long)?;
    write_png(&base.join("picture.png"), 240, 120)?;
    Ok(base)
}

/// A gradient as a PNG of `width` × `height`, deflated without compression.
fn write_png(path: &Path, width: u32, height: u32) -> std::io::Result<()> {
    let mut raw = Vec::new();
    for y in 0..height {
        raw.push(0);
        for x in 0..width {
            let red = u8::try_from(x * 255 / width.max(1)).unwrap_or(u8::MAX);
            let green = u8::try_from(y * 255 / height.max(1)).unwrap_or(u8::MAX);
            raw.extend([red, green, 200]);
        }
    }
    let mut zlib = vec![0x78, 0x01];
    let blocks: Vec<&[u8]> = raw.chunks(65_535).collect();
    for (n, block) in blocks.iter().enumerate() {
        zlib.push(u8::from(n + 1 == blocks.len()));
        let len = u16::try_from(block.len()).unwrap_or(u16::MAX);
        zlib.extend(len.to_le_bytes());
        zlib.extend((!len).to_le_bytes());
        zlib.extend_from_slice(block);
    }
    zlib.extend(adler32(&raw).to_be_bytes());
    let mut header = Vec::new();
    header.extend(width.to_be_bytes());
    header.extend(height.to_be_bytes());
    header.extend([8, 2, 0, 0, 0]);
    let mut png = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    for (kind, data) in [(b"IHDR", header), (b"IDAT", zlib), (b"IEND", Vec::new())] {
        png.extend(u32::try_from(data.len()).unwrap_or(u32::MAX).to_be_bytes());
        let mut chunk = kind.to_vec();
        chunk.extend(&data);
        png.extend(&chunk);
        png.extend(crc32(&chunk).to_be_bytes());
    }
    std::fs::write(path, png)
}

fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for &byte in data {
        a = (a + u32::from(byte)) % 65_521;
        b = (b + a) % 65_521;
    }
    (b << 16) | a
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &byte in data {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}
