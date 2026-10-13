fn main() -> std::process::ExitCode {
    match execute() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("fastdiff: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}
fn execute() -> fastdiff_cli::Result<()> {
    use fastdiff_cli::{Command, HELP, launch, parse, run_stdio_helper_target};
    match parse(std::env::args_os().skip(1))? {
        Command::Help => {
            print!("{HELP}");
            Ok(())
        }
        Command::Version => {
            println!(
                "fastdiff {} (helper protocol {})",
                env!("CARGO_PKG_VERSION"),
                fastdiff_cli::PROTOCOL_VERSION
            );
            Ok(())
        }
        Command::Helper { repository, target } => run_stdio_helper_target(&repository, target),
        Command::Open { request, native } => launch(request, native),
    }
}
