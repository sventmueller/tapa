//! The selection as RTF and HTML with its colours (F114), and plain text without the blanks that end its lines.

use std::fmt::Write;

use term_session::Rgb;
use term_session::session::{Look, StyledText};

/// `text` without the blanks at the end of each line.
pub(crate) fn trimmed(text: &str) -> String {
    let lines: Vec<&str> = text
        .split('\n')
        .map(|line| line.trim_end_matches([' ', '\t']))
        .collect();
    lines.join("\n")
}

/// `text` as RTF in the font `family` at `size` points, each run in its colours on its background.
pub(crate) fn rtf(text: &StyledText, family: &str, size: f64) -> String {
    let mut colours = vec![text.foreground, text.background];
    let mut index = |rgb: Rgb| match colours.iter().position(|&c| c == rgb) {
        Some(at) => at + 1,
        None => {
            colours.push(rgb);
            colours.len()
        }
    };
    let mut body = String::new();
    for (number, line) in text.lines.iter().enumerate() {
        if number > 0 {
            body.push_str("\\line\n");
        }
        for run in line {
            let (fg, bg) = (index(run.look.fg), index(run.look.bg));
            let _ = write!(body, "{{\\cf{fg}\\cb{bg}\\highlight{bg}{} ", switches(run.look));
            escape_rtf(&run.text, &mut body);
            body.push('}');
        }
    }
    let mut table = String::new();
    for rgb in &colours {
        let _ = write!(table, "\\red{}\\green{}\\blue{};", rgb.r, rgb.g, rgb.b);
    }
    let half_points = (size * 2.0).round().clamp(2.0, 2000.0);
    format!(
        "{{\\rtf1\\ansi\\ansicpg1252\\deff0{{\\fonttbl{{\\f0\\fmodern {};}}}}{{\\colortbl;{table}}}\n\\f0\\fs{half_points}\\cf1\\cb2 {body}}}",
        rtf_name(family)
    )
}

fn switches(look: Look) -> String {
    let mut out = String::new();
    for (on, switch) in [
        (look.bold, "\\b"),
        (look.italic, "\\i"),
        (look.underline, "\\ul"),
        (look.strike, "\\strike"),
    ] {
        if on {
            out.push_str(switch);
        }
    }
    out
}

fn rtf_name(family: &str) -> String {
    family.chars().filter(|c| !"\\{};".contains(*c)).collect()
}

/// RTF wants characters outside ASCII as signed 16-bit UTF-16 units, each with a `?` for readers that know none.
fn escape_rtf(text: &str, out: &mut String) {
    for c in text.chars() {
        match c {
            '\\' | '{' | '}' => {
                out.push('\\');
                out.push(c);
            }
            ' '..='~' => out.push(c),
            _ => {
                let mut units = [0u16; 2];
                for unit in c.encode_utf16(&mut units) {
                    let _ = write!(out, "\\u{}?", i16::from_ne_bytes(unit.to_ne_bytes()));
                }
            }
        }
    }
}

/// `text` as an HTML fragment in the font `family` at `size` points, on the terminal's background.
pub(crate) fn html(text: &StyledText, family: &str, size: f64) -> String {
    let mut out = format!(
        "<meta charset=\"utf-8\"><pre style=\"font-family: '{}', monospace; font-size: {size}pt; color: {}; background-color: {}; padding: 6pt;\">",
        escape_html(&family.replace('\'', "")),
        hex(text.foreground),
        hex(text.background)
    );
    for (number, line) in text.lines.iter().enumerate() {
        if number > 0 {
            out.push('\n');
        }
        for run in line {
            let look = run.look;
            let mut style = String::new();
            if look.fg != text.foreground {
                let _ = write!(style, "color: {};", hex(look.fg));
            }
            if look.bg != text.background {
                let _ = write!(style, "background-color: {};", hex(look.bg));
            }
            if look.bold {
                style.push_str("font-weight: bold;");
            }
            if look.italic {
                style.push_str("font-style: italic;");
            }
            let lines = match (look.underline, look.strike) {
                (true, true) => "underline line-through",
                (true, false) => "underline",
                (false, true) => "line-through",
                (false, false) => "",
            };
            if !lines.is_empty() {
                let _ = write!(style, "text-decoration: {lines};");
            }
            let escaped = escape_html(&run.text);
            if style.is_empty() {
                out.push_str(&escaped);
            } else {
                let _ = write!(out, "<span style=\"{style}\">{escaped}</span>");
            }
        }
    }
    out.push_str("</pre>");
    out
}

fn hex(rgb: Rgb) -> String {
    format!("#{:02x}{:02x}{:02x}", rgb.r, rgb.g, rgb.b)
}

fn escape_html(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use term_session::session::StyledRun;

    use super::*;

    const WHITE: Rgb = Rgb::new(255, 255, 255);
    const BLACK: Rgb = Rgb::new(0, 0, 0);
    const RED: Rgb = Rgb::new(200, 0, 0);

    fn plain() -> Look {
        Look {
            fg: WHITE,
            bg: BLACK,
            bold: false,
            italic: false,
            underline: false,
            strike: false,
        }
    }

    fn sample() -> StyledText {
        let run = |text: &str, look| StyledRun {
            text: text.into(),
            look,
        };
        StyledText {
            lines: vec![
                vec![
                    run("ok ", plain()),
                    run(
                        "<fail>",
                        Look {
                            fg: RED,
                            bold: true,
                            ..plain()
                        },
                    ),
                ],
                vec![run("{ä} 🙂\\", plain())],
            ],
            foreground: WHITE,
            background: BLACK,
        }
    }

    #[test]
    fn plain_text_loses_the_blanks_at_the_end_of_its_lines() {
        assert_eq!(trimmed("a  \n b\t\n\nc "), "a\n b\n\nc");
    }

    #[test]
    fn rtf_has_a_colour_table_and_escapes_what_it_must() {
        let rtf = rtf(&sample(), "SF Mono", 13.0);
        assert!(rtf.starts_with("{\\rtf1"), "{rtf}");
        assert!(rtf.contains("{\\fonttbl{\\f0\\fmodern SF Mono;}}"), "{rtf}");
        assert!(
            rtf.contains("{\\colortbl;\\red255\\green255\\blue255;\\red0\\green0\\blue0;\\red200\\green0\\blue0;}"),
            "{rtf}"
        );
        assert!(rtf.contains("\\fs26"), "{rtf}");
        assert!(rtf.contains("{\\cf3\\cb2\\highlight2\\b <fail>}"), "{rtf}");
        assert!(rtf.contains("\\{\\u228?\\} \\u-10179?\\u-8638?\\\\"), "{rtf}");
        assert!(rtf.ends_with('}'));
        let mut depth = 0;
        let mut chars = rtf.chars();
        while let Some(c) = chars.next() {
            match c {
                '\\' => {
                    chars.next();
                }
                '{' => depth += 1,
                '}' => depth -= 1,
                _ => {}
            }
            assert!(depth >= 0, "{rtf}");
        }
        assert_eq!(depth, 0, "{rtf}");
    }

    #[test]
    fn html_styles_only_the_runs_that_differ_from_the_terminal() {
        let html = html(&sample(), "SF Mono", 13.0);
        assert!(html.contains("font-family: 'SF Mono', monospace"), "{html}");
        assert!(
            html.contains("ok <span style=\"color: #c80000;font-weight: bold;\">&lt;fail&gt;</span>\n{ä} 🙂\\</pre>"),
            "{html}"
        );
    }
}
