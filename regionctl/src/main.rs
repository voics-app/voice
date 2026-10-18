//! Composition root of `regionctl`.

use std::process::ExitCode;

fn main() -> ExitCode {
    regionctl::cli::run(std::env::args_os(), |name| std::env::var(name).ok())
}
