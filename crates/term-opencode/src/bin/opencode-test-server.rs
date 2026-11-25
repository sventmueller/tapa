//! The stand-in of `term_opencode::test_server` as an `opencode` executable: `--version` prints the release, `serve`
//! serves the API on 127.0.0.1 until its standard input closes.

use term_opencode::test_server;

fn main() {
    let scenario = std::env::var(test_server::SCENARIO_VARIABLE).unwrap_or_else(|_| "basic".into());
    if std::env::args().any(|a| a == "--version") {
        println!("{}", test_server::version(&scenario));
        return;
    }
    let log = std::env::var_os(test_server::LOG_VARIABLE).map(std::path::PathBuf::from);
    test_server::run(std::io::stdin(), std::io::stdout().lock(), &scenario, log);
}
