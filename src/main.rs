use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use decisionbench::dataset::{EFFORTLESS_SOURCE, EFFORTLESS_SOURCE_NAME, parse_source};

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
    /// Validate a source dataset and print a summary (offline).
    Inspect {
        /// Source fixture to inspect; defaults to the bundled effortless dataset.
        path: Option<PathBuf>,
    },
}

fn main() -> ExitCode {
    let Command::Dataset(DatasetCommand::Inspect { path }) = Cli::parse().command;
    let (name, bytes) = match &path {
        None => (
            EFFORTLESS_SOURCE_NAME.to_string(),
            EFFORTLESS_SOURCE.to_vec(),
        ),
        Some(p) => match std::fs::read(p) {
            Ok(bytes) => (p.display().to_string(), bytes),
            Err(e) => {
                eprintln!("error: cannot read {}: {e}", p.display());
                return ExitCode::FAILURE;
            }
        },
    };
    match parse_source(&bytes) {
        Ok(dataset) => {
            print!("dataset: {name}\n{}", dataset.summary());
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("error: invalid dataset {name}: {e}");
            ExitCode::FAILURE
        }
    }
}
