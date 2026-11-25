//! The text of an earlier session written into a new one's screen before its program starts (F71): faint, so it
//! reads as the past, and stripped of anything a terminal would take as a command (F182).

use crate::fair::FairMutex;
use crate::own::Own;

/// Writes `rows` into `screen`, each on its own line, the cursor left at the start of the next.
pub(super) fn replay(screen: &FairMutex<Own>, rows: &[String]) {
    if rows.is_empty() {
        return;
    }
    screen.lock().terminal.advance(&bytes_of(rows));
}

/// The rows as output: faint, with control characters dropped, each ended by CR LF.
fn bytes_of(rows: &[String]) -> Vec<u8> {
    let mut out = String::from("\x1b[2m");
    for row in rows {
        out.extend(row.chars().filter(|c| !c.is_control()));
        out.push_str("\r\n");
    }
    out.push_str("\x1b[0m");
    out.into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restored_text_cannot_carry_commands() {
        let rows = vec!["ok".to_string(), "\x1b]52;c;?\x07evil\u{9b}2J".to_string()];
        let text = String::from_utf8(bytes_of(&rows)).unwrap();
        assert_eq!(text, "\x1b[2mok\r\n]52;c;?evil2J\r\n\x1b[0m");
    }

    #[test]
    fn the_own_core_shows_the_rows_above_the_cursor() {
        let terminal = term_vt::Terminal::new(term_vt::Size { columns: 20, lines: 5 }, 100);
        let screen = FairMutex::new(Own::new(terminal));
        replay(&screen, &["first".into(), "second".into()]);
        let own = screen.lock();
        let text = own.visible_text();
        assert_eq!(text[0].trim_end(), "first");
        assert_eq!(text[1].trim_end(), "second");
        assert_eq!(own.terminal.cursor().line, 2);
    }
}
