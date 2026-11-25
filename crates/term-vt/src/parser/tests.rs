use super::*;

/// What a parse did, written out.
#[derive(Default)]
struct Log(Vec<String>);

impl Perform for Log {
    fn print(&mut self, c: char) {
        self.0.push(format!("print {c}"));
    }

    fn print_ascii(&mut self, run: &[u8]) {
        self.0.push(format!("ascii {}", String::from_utf8_lossy(run)));
    }

    fn execute(&mut self, byte: u8) {
        self.0.push(format!("exec {byte:#04x}"));
    }

    fn csi(&mut self, params: &Params, intermediates: &[u8], private: Option<u8>, action: u8) {
        let params: Vec<String> = params
            .iter()
            .map(|values| values.iter().map(u16::to_string).collect::<Vec<_>>().join(":"))
            .collect();
        self.0.push(format!(
            "csi {}{}{} {}",
            private.map(char::from).map(String::from).unwrap_or_default(),
            params.join(";"),
            String::from_utf8_lossy(intermediates),
            char::from(action)
        ));
    }

    fn esc(&mut self, intermediates: &[u8], byte: u8) {
        self.0.push(format!(
            "esc {}{}",
            String::from_utf8_lossy(intermediates),
            char::from(byte)
        ));
    }

    fn osc(&mut self, params: &[&[u8]], bell: bool) {
        let parts: Vec<String> = params.iter().map(|p| String::from_utf8_lossy(p).into_owned()).collect();
        self.0
            .push(format!("osc {} {}", parts.join("|"), if bell { "bel" } else { "st" }));
    }

    fn dcs(&mut self, params: &Params, _intermediates: &[u8], private: Option<u8>, action: u8, data: &[u8]) {
        self.0.push(format!(
            "dcs {}{} {} {}",
            private.map(char::from).map(String::from).unwrap_or_default(),
            params.len(),
            char::from(action),
            String::from_utf8_lossy(data)
        ));
    }
}

fn parse(chunks: &[&[u8]]) -> Vec<String> {
    let mut parser = Parser::new();
    let mut log = Log::default();
    for chunk in chunks {
        parser.advance(&mut log, chunk);
    }
    log.0
}

#[test]
fn text_runs_and_controls() {
    assert_eq!(
        parse(&[b"ab\r\ncd"]),
        ["ascii ab", "exec 0x0d", "exec 0x0a", "ascii cd"]
    );
}

#[test]
fn utf8_across_reads_and_broken_characters() {
    assert_eq!(parse(&["ä€".as_bytes()]), ["print ä", "print €"]);
    let euro = "€".as_bytes();
    assert_eq!(parse(&[&euro[..1], &euro[1..2], &euro[2..]]), ["print €"]);
    assert_eq!(parse(&[b"\xe2\x82A"]), ["print \u{fffd}", "print A"]);
    assert_eq!(parse(&[b"\xc0\xff"]), ["print \u{fffd}", "print \u{fffd}"]);
}

#[test]
fn control_sequences_with_parameters_subparameters_and_markers() {
    assert_eq!(parse(&[b"\x1b[m"]), ["csi  m"]);
    assert_eq!(parse(&[b"\x1b[;5H"]), ["csi 0;5 H"]);
    assert_eq!(parse(&[b"\x1b[1;H"]), ["csi 1;0 H"]);
    assert_eq!(parse(&[b"\x1b[38:2::255:0:0m"]), ["csi 38:2:0:255:0:0 m"]);
    assert_eq!(parse(&[b"\x1b[?1049h"]), ["csi ?1049 h"]);
    assert_eq!(parse(&[b"\x1b[>0c"]), ["csi >0 c"]);
    assert_eq!(parse(&[b"\x1b[2 q"]), ["csi 2  q"]);
    assert_eq!(parse(&[b"\x1b[3", b"1m"]), ["csi 31 m"]);
}

#[test]
fn controls_inside_a_sequence_run_and_cancel_aborts_it() {
    assert_eq!(parse(&[b"\x1b[1\n2H"]), ["exec 0x0a", "csi 12 H"]);
    assert_eq!(parse(&[b"\x1b[12\x18x"]), ["exec 0x18", "ascii x"]);
}

#[test]
fn escape_sequences() {
    assert_eq!(parse(&[b"\x1b7\x1b(0\x1b#8"]), ["esc 7", "esc (0", "esc #8"]);
}

#[test]
fn osc_with_either_terminator_and_semicolons_in_its_text() {
    assert_eq!(parse(&[b"\x1b]0;a;b\x07"]), ["osc 0|a|b bel"]);
    assert_eq!(parse(&[b"\x1b]8;;http://x\x1b\\"]), ["osc 8||http://x st"]);
    assert_eq!(parse(&[b"\x1b]2;t\x1b[m"]), ["osc 2|t st", "csi  m"]);
}

#[test]
fn dcs_carries_its_data_and_other_strings_are_skipped() {
    assert_eq!(parse(&[b"\x1bP+q544e\x1b\\"]), ["dcs 0 q 544e"]);
    assert_eq!(parse(&[b"\x1b_Gf=24;AAAA\x1b\\x"]), ["ascii x"]);
}

#[test]
fn a_sequence_reads_the_same_wherever_the_read_ends() {
    let output: &[u8] = b"a\x1b[2;3H\x1b[Hb\x1b(0\x1b[?25l\xc3\xa4\x1b[1 q\x1b\x1b[m";
    let whole = parse(&[output]);
    assert_eq!(
        whole,
        [
            "ascii a",
            "csi 2;3 H",
            "csi  H",
            "ascii b",
            "esc (0",
            "csi ?25 l",
            "print ä",
            "csi 1  q",
            "csi  m"
        ]
    );
    for cut in 0..=output.len() {
        assert_eq!(parse(&[&output[..cut], &output[cut..]]), whole, "cut at {cut}");
    }
}
