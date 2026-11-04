use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use decisionbench::dataset::{BUNDLED_CASES, parse_cases, summarize};

#[derive(Parser)]
#[command(version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Work with benchmark datasets.
    #[command(subcommand)]
    Dataset(DatasetCommand),
}

#[derive(Subcommand)]
enum DatasetCommand {
    /// Validate a dataset file and print a summary (offline).
    Inspect {
        /// Dataset file to inspect; defaults to the bundled dataset.
        path: Option<PathBuf>,
    },
}

fn main() -> ExitCode {
    let Command::Dataset(DatasetCommand::Inspect { path }) = Cli::parse().command;
    let (name, bytes) = match &path {
        None => ("bundled cases".to_string(), BUNDLED_CASES.to_vec()),
        Some(p) => match std::fs::read(p) {
            Ok(bytes) => (p.display().to_string(), bytes),
            Err(e) => {
                eprintln!("error: cannot read {}: {e}", p.display());
                return ExitCode::FAILURE;
            }
        },
    };
    match parse_cases(&bytes) {
        Ok(cases) => {
            print!("dataset: {name}\n{}", summarize(&cases));
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("error: invalid dataset {name}: {e}");
            ExitCode::FAILURE
        }
    }
}
