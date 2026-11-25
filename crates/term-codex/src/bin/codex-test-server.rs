//! The stand-in of `term_codex::test_server` as a `codex` executable: `--version` prints the release, `app-server`
//! speaks the protocol on stdin and stdout.

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    std::process::exit(term_codex::test_server::main(&args));
}
