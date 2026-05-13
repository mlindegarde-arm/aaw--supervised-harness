use std::process::ExitCode;

fn main() -> ExitCode {
    let exit = harness::cli::run(std::env::args());
    ExitCode::from(exit.code())
}
