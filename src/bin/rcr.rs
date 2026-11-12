use clap::Parser;
use rust_crate_release::cli::{Cli, run};
use std::process::ExitCode;

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(output) => {
            println!("{output}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("rcr: {error}");
            ExitCode::FAILURE
        }
    }
}
