use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use clap::{Parser, Subcommand};
use decisionbench::batch::{self, Control, RunArgs, load_dataset, load_run, read};
use decisionbench::cost::plural;
use decisionbench::dataset::summarize;
use decisionbench::evidence::{self, Line, RunFile, RunState};
use decisionbench::grade::{Tally, Target, grade_all, parse_predictions};
use decisionbench::report::{self, Axis};
use decisionbench::run::Settings;

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
    /// Run a batch of Trials (Candidate × instructions × Case × repetition), save the
    /// evidence and print a summary.
    ///
    /// Exits 3 when some Trial ended in a provider or transport failure (evidence is still saved).
    Run {
        /// Candidate name from the config file; repeatable.
        #[arg(long = "candidate", required = true)]
        candidates: Vec<String>,
        /// Judge Instructions file; repeatable. Defaults to the bundled `neutral`.
        #[arg(long = "instructions")]
        instructions: Vec<PathBuf>,
        /// Case id in the dataset; repeatable. Defaults to every Case.
        #[arg(long = "case")]
        cases: Vec<String>,
        /// Target Model to ask about; repeatable. Defaults to all four.
        #[arg(long = "target", value_enum)]
        targets: Vec<Target>,
        /// Repetitions of every combination.
        #[arg(long, default_value_t = 3, value_parser = clap::value_parser!(u32).range(1..))]
        repetitions: u32,
        /// Trials in flight at once.
        #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u32).range(1..))]
        concurrency: u32,
        /// Seconds before one request gives up.
        #[arg(long = "timeout", default_value_t = 60, value_parser = clap::value_parser!(u64).range(1..))]
        timeout_secs: u64,
        /// Extra attempts after a transport error or a transient HTTP status.
        #[arg(long, default_value_t = 1)]
        retries: u32,
        /// Also run Cases that none of the selected Targets labels.
        #[arg(long)]
        include_unscored: bool,
        /// Config file; defaults to $XDG_CONFIG_HOME/decisionbench/config.toml.
        #[arg(long)]
        config: Option<PathBuf>,
        /// Dataset file; defaults to the bundled dataset.
        #[arg(long)]
        dataset: Option<PathBuf>,
        /// Evidence directory; defaults to $XDG_DATA_HOME/decisionbench/runs.
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Print the full report of a saved run file (offline).
    Show {
        file: PathBuf,
        /// What the summary table has as columns; auto picks prompts or candidates, whichever differ.
        #[arg(long, value_enum, default_value_t = Axis::Auto)]
        columns: Axis,
    },
    /// Put several saved run files side by side and list what makes them not comparable (offline).
    Compare {
        #[arg(required = true, num_args = 2..)]
        files: Vec<PathBuf>,
        /// What the table has as columns; auto picks prompts or candidates, whichever differ.
        #[arg(long, value_enum, default_value_t = Axis::Auto)]
        columns: Axis,
    },
    /// Open the terminal UI: manage candidates, inspect the dataset, run, and read results.
    Tui {
        /// Config file; defaults to $XDG_CONFIG_HOME/decisionbench/config.toml.
        #[arg(long)]
        config: Option<PathBuf>,
        /// Evidence directory; defaults to $XDG_DATA_HOME/decisionbench/runs.
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// List saved runs by created time (offline).
    History {
        /// Evidence directory; defaults to $XDG_DATA_HOME/decisionbench/runs.
        dir: Option<PathBuf>,
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
            load_dataset(path.as_deref()).map(|(name, _, cases)| {
                print!("dataset: {name}\n{}", summarize(&cases));
                ExitCode::SUCCESS
            })
        }
        Command::Grade {
            predictions,
            dataset,
        } => grade(&predictions, dataset.as_deref()).map(|()| ExitCode::SUCCESS),
        Command::Run {
            candidates,
            instructions,
            cases,
            targets,
            repetitions,
            concurrency,
            timeout_secs,
            retries,
            include_unscored,
            config,
            dataset,
            out,
        } => run_batch(&RunArgs {
            candidates,
            instructions: if instructions.is_empty() {
                vec![None]
            } else {
                instructions.into_iter().map(Some).collect()
            },
            cases,
            targets,
            settings: Settings {
                repetitions,
                concurrency,
                timeout_secs,
                retries,
                include_unscored,
            },
            config,
            dataset,
            out,
        }),
        Command::Show { file, columns } => show(&file, columns).map(|()| ExitCode::SUCCESS),
        Command::Compare { files, columns } => compare(&files, columns).map(|()| ExitCode::SUCCESS),
        Command::Tui { config, out } => {
            decisionbench::tui::run(config, out).map(|()| ExitCode::SUCCESS)
        }
        Command::History { dir } => history(dir.as_deref()).map(|()| ExitCode::SUCCESS),
    };
    match result {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn grade(path: &Path, dataset: Option<&Path>) -> Result<(), String> {
    let (name, _, cases) = load_dataset(dataset)?;
    let predictions = parse_predictions(&read(path)?)
        .map_err(|e| format!("invalid predictions {}: {e}", path.display()))?;
    let grades = grade_all(&cases, &predictions)
        .map_err(|e| format!("invalid predictions {}: {e}", path.display()))?;
    let mut tallies: BTreeMap<Target, Tally> = BTreeMap::new();
    println!("dataset: {name}");
    for (p, g) in predictions.iter().zip(grades) {
        println!("{} {} {} {g}", p.case, p.target, p.effort);
        tallies.entry(p.target).or_default().add(g);
    }
    for target in Target::ALL {
        println!("{target}: {}", tallies.remove(&target).unwrap_or_default());
    }
    Ok(())
}

/// Every input is checked before the first request (see `batch::prepare`). Ctrl-C asks the Run to stop taking new Trials.
fn run_batch(args: &RunArgs) -> Result<ExitCode, String> {
    let prepared = batch::prepare(args)?;
    eprintln!("{}", prepared.plan_line());
    let path = prepared.path.clone();
    let planned = prepared.planned();

    let control = Arc::new(Control::default());
    {
        let control = control.clone();
        let signals = AtomicUsize::new(0);
        ctrlc::set_handler(move || {
            if signals.fetch_add(1, Ordering::Relaxed) > 0 {
                std::process::exit(130);
            }
            control.cancel.store(true, Ordering::Relaxed);
            eprintln!(
                "cancelling: waiting for {}; press Ctrl-C again to stop now",
                plural(control.in_flight(), "in-flight trial", "in-flight trials")
            );
        })
        .map_err(|e| format!("cannot install the Ctrl-C handler: {e}"))?;
    }
    let outcome = prepared.run(&control, |trial, done, unsaved| {
        eprintln!("{}", trial.progress(done, planned));
        if unsaved {
            eprint!("{}", evidence::printable(&Line::Trial(trial.clone())));
        }
    });

    print!("{}", outcome.summary);
    println!("evidence: {}", path.display());
    if let Some(e) = outcome.write_error {
        return Err(e);
    }
    Ok(match outcome.state {
        RunState::Complete if outcome.all_answered => ExitCode::SUCCESS,
        RunState::Complete => ExitCode::from(3),
        RunState::Partial => ExitCode::from(130),
        RunState::Failed => ExitCode::FAILURE,
    })
}

fn show(path: &Path, columns: Axis) -> Result<(), String> {
    print!("{}", report::report(&load_run(path)?, columns));
    Ok(())
}

fn compare(paths: &[PathBuf], columns: Axis) -> Result<(), String> {
    let runs: Vec<RunFile> = paths
        .iter()
        .map(|p| load_run(p))
        .collect::<Result<_, _>>()?;
    for warning in runs.iter().flat_map(|r| &r.warnings) {
        eprintln!("warning: {warning}");
    }
    print!("{}", report::Comparison(&runs, columns));
    Ok(())
}

fn history(dir: Option<&Path>) -> Result<(), String> {
    let dir = batch::runs_dir(dir)?;
    let entries = batch::read_runs(&dir)?;
    if entries.is_empty() {
        println!("no runs in {}", dir.display());
    } else {
        print!("{}", report::history(entries));
    }
    Ok(())
}
