//! The stand-ins of `term_codex::test_server`, `term_opencode::test_server` and `term_acp::test_agent` as one program,
//! for the driver's tests: `SESSION_STAND_IN` names which; the arguments are those of `codex` or `opencode`, and for
//! the ACP agent the first one names its scenario.

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match std::env::var("SESSION_STAND_IN").as_deref() {
        Ok("codex") => std::process::exit(term_codex::test_server::main(&args)),
        Ok("opencode") => {
            let scenario =
                std::env::var(term_opencode::test_server::SCENARIO_VARIABLE).unwrap_or_else(|_| "basic".into());
            if args.iter().any(|a| a == "--version") {
                println!("{}", term_opencode::test_server::version(&scenario));
                return;
            }
            let log = std::env::var_os(term_opencode::test_server::LOG_VARIABLE).map(std::path::PathBuf::from);
            term_opencode::test_server::run(std::io::stdin(), std::io::stdout().lock(), &scenario, log);
        }
        Ok("acp") => {
            let scenario = args.first().map_or("basic", String::as_str);
            term_acp::test_agent::run(std::io::stdin().lock(), std::io::stdout().lock(), scenario);
        }
        _ => {
            eprintln!("session-stand-in: set SESSION_STAND_IN to codex, opencode or acp");
            std::process::exit(2);
        }
    }
}
