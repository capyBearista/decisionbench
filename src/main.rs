use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use decisionbench::dataset::{BUNDLED_CASES, Case, parse_cases, summarize};
use decisionbench::grade::{Tally, Target, grade_all, parse_predictions};

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
    /// Grade raw effort predictions from a local JSON file against dataset ranges (offline).
    Grade {
        /// JSON array of `{"case", "target", "effort"}` predictions.
        predictions: PathBuf,
        /// Dataset file to grade against; defaults to the bundled dataset.
        #[arg(long)]
        dataset: Option<PathBuf>,
    },
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
    let result = match Cli::parse().command {
        Command::Dataset(DatasetCommand::Inspect { path }) => {
            load_dataset(path.as_deref()).map(|(name, cases)| {
                print!("dataset: {name}\n{}", summarize(&cases));
            })
        }
        Command::Grade {
            predictions,
            dataset,
        } => grade(&predictions, dataset.as_deref()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn read(path: &Path) -> Result<Vec<u8>, String> {
    std::fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))
}

fn load_dataset(path: Option<&Path>) -> Result<(String, Vec<Case>), String> {
    let (name, bytes) = match path {
        None => ("bundled cases".to_string(), BUNDLED_CASES.to_vec()),
        Some(p) => (p.display().to_string(), read(p)?),
    };
    let cases = parse_cases(&bytes).map_err(|e| format!("invalid dataset {name}: {e}"))?;
    Ok((name, cases))
}

fn grade(path: &Path, dataset: Option<&Path>) -> Result<(), String> {
    let (name, cases) = load_dataset(dataset)?;
    let predictions = parse_predictions(&read(path)?)
        .map_err(|e| format!("invalid predictions {}: {e}", path.display()))?;
    let grades = grade_all(&cases, &predictions)
        .map_err(|e| format!("invalid predictions {}: {e}", path.display()))?;
    let (mut sonnet, mut opus) = (Tally::default(), Tally::default());
    println!("dataset: {name}");
    for (p, g) in predictions.iter().zip(grades) {
        println!("{} {} {} {g}", p.case, p.target, p.effort);
        match p.target {
            Target::Sonnet => sonnet.add(g),
            Target::Opus => opus.add(g),
        }
    }
    println!("sonnet: {sonnet}\nopus: {opus}");
    Ok(())
}
