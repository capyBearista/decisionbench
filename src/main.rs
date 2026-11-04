use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};

use clap::{Parser, Subcommand};
use decisionbench::cost::plural;
use decisionbench::dataset::{BUNDLED_CASES, Case, parse_cases, summarize};
use decisionbench::evidence::{
    self, CandidateRecord, EndRecord, GRADING_REVISION, Hashed, InstructionsRecord, Line, Manifest,
    RunFile, RunState,
};
use decisionbench::grade::{Tally, Target, grade_all, parse_predictions};
use decisionbench::instructions::{
    BUNDLED_NAME, BUNDLED_TEXT, JudgeInstructions, parse_instructions,
};
use decisionbench::provider::{
    Candidate, env_nonempty, now_unix_ms, parse_candidates, request_url,
};
use decisionbench::report;
use decisionbench::run::{self, Job, Settings};
use decisionbench::sha256_hex;

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
    Show { file: PathBuf },
    /// Put several saved run files side by side and list what makes them not comparable (offline).
    Compare {
        #[arg(required = true, num_args = 2..)]
        files: Vec<PathBuf>,
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
            instructions,
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
        Command::Show { file } => show(&file).map(|()| ExitCode::SUCCESS),
        Command::Compare { files } => compare(&files).map(|()| ExitCode::SUCCESS),
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

fn read(path: &Path) -> Result<Vec<u8>, String> {
    std::fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))
}

/// The dataset's display name, SHA-256 of its bytes (its revision) and Cases.
fn load_dataset(path: Option<&Path>) -> Result<(String, String, Vec<Case>), String> {
    let (name, bytes) = match path {
        None => ("bundled cases".to_string(), BUNDLED_CASES.to_vec()),
        Some(p) => (p.display().to_string(), read(p)?),
    };
    let cases = parse_cases(&bytes).map_err(|e| format!("invalid dataset {name}: {e}"))?;
    Ok((name, sha256_hex(&bytes), cases))
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

/// `$XDG_*` if set, else `$HOME/<home_rel>`, then `decisionbench/<rest>`.
fn default_path(xdg_var: &str, home_rel: &str, rest: &str) -> Result<PathBuf, String> {
    env_nonempty(xdg_var)
        .map(PathBuf::from)
        .or_else(|| env_nonempty("HOME").map(|home| Path::new(&home).join(home_rel)))
        .map(|base| base.join("decisionbench").join(rest))
        .ok_or_else(|| format!("cannot find ${xdg_var} or $HOME; pass the path explicitly"))
}

fn load_instructions(path: Option<&Path>) -> Result<JudgeInstructions, String> {
    let Some(path) = path else {
        return parse_instructions(BUNDLED_NAME, BUNDLED_TEXT);
    };
    let text = String::from_utf8(read(path)?)
        .map_err(|_| format!("invalid instructions {}: not UTF-8", path.display()))?;
    let name = path
        .file_stem()
        .map_or_else(String::new, |s| s.to_string_lossy().into_owned());
    parse_instructions(&name, &text)
        .map_err(|e| format!("invalid instructions {}: {e}", path.display()))
}

struct RunArgs {
    candidates: Vec<String>,
    instructions: Vec<PathBuf>,
    cases: Vec<String>,
    targets: Vec<Target>,
    settings: Settings,
    config: Option<PathBuf>,
    dataset: Option<PathBuf>,
    out: Option<PathBuf>,
}

fn reject_duplicates(kind: &str, items: impl IntoIterator<Item = String>) -> Result<(), String> {
    let mut seen = HashSet::new();
    match items.into_iter().find(|item| !seen.insert(item.clone())) {
        Some(item) => Err(format!("duplicate {kind} `{item}`")),
        None => Ok(()),
    }
}

/// Every input is checked, and the evidence file is created, before the first request, so an input
/// or output error sends nothing. The main thread is the only writer: workers send finished
/// Trials over a channel and each becomes one line.
fn run_batch(args: &RunArgs) -> Result<ExitCode, String> {
    let config = match &args.config {
        Some(path) => path.clone(),
        None => default_path("XDG_CONFIG_HOME", ".config", "config.toml")?,
    };
    let out = match &args.out {
        Some(path) => path.clone(),
        None => default_path("XDG_DATA_HOME", ".local/share", "runs")?,
    };
    let text = String::from_utf8(read(&config)?)
        .map_err(|_| format!("invalid config {}: not UTF-8", config.display()))?;
    let configured =
        parse_candidates(&text).map_err(|e| format!("invalid config {}: {e}", config.display()))?;
    reject_duplicates("candidate", args.candidates.iter().cloned())?;
    let candidates: Vec<Candidate> = args
        .candidates
        .iter()
        .map(|name| {
            configured
                .iter()
                .find(|c| &c.name == name)
                .cloned()
                .ok_or_else(|| format!("unknown candidate `{name}` in {}", config.display()))
        })
        .collect::<Result<_, _>>()?;

    let instructions = if args.instructions.is_empty() {
        vec![load_instructions(None)?]
    } else {
        args.instructions
            .iter()
            .map(|path| load_instructions(Some(path)))
            .collect::<Result<_, _>>()?
    };
    reject_duplicates(
        "instructions name",
        instructions.iter().map(|i| i.name.clone()),
    )?;

    let (dataset_name, dataset_sha256, mut cases) = load_dataset(args.dataset.as_deref())?;
    reject_duplicates("case", args.cases.iter().cloned())?;
    if let Some(id) = args
        .cases
        .iter()
        .find(|id| !cases.iter().any(|c| &c.id == *id))
    {
        return Err(format!("unknown case {id} in {dataset_name}"));
    }
    if !args.cases.is_empty() {
        cases.retain(|c| args.cases.contains(&c.id));
    }
    reject_duplicates("target", args.targets.iter().map(Target::to_string))?;
    // Canonical order, so the request, the evidence and the summary all list targets alike.
    let mut targets = if args.targets.is_empty() {
        Target::ALL.to_vec()
    } else {
        args.targets.clone()
    };
    targets.sort();
    for set in &instructions {
        if let Some(target) = targets.iter().find(|t| set.question(**t).is_none()) {
            return Err(format!(
                "instructions `{}` has no section for target {target}",
                set.name
            ));
        }
    }

    let keys: Vec<Option<String>> = candidates
        .iter()
        .map(|c| c.provider.key())
        .collect::<Result<_, _>>()?;
    let plan = run::plan(
        candidates.len(),
        instructions.len(),
        &cases,
        &targets,
        &args.settings,
    );
    if plan.trials.is_empty() {
        return Err(
            "no trials: no selected Case is labelled for a selected Target; pass --include-unscored to run them anyway"
                .to_string(),
        );
    }

    let id = format!("{}-{}", now_unix_ms(), std::process::id());
    let (mut file, path) = evidence::create(&out, &id)?;
    let urls: Vec<String> = candidates.iter().map(|c| request_url(c.provider)).collect();
    let planned = plan.trials.len();
    let manifest = Manifest {
        schema_version: evidence::SCHEMA_VERSION,
        id,
        created_unix_ms: now_unix_ms(),
        dataset: Hashed {
            name: dataset_name,
            sha256: dataset_sha256,
        },
        instructions: instructions
            .iter()
            .map(|i| InstructionsRecord {
                name: i.name.clone(),
                sha256: i.sha256.clone(),
                text: i.text.clone(),
            })
            .collect(),
        grading_revision: GRADING_REVISION,
        candidates: candidates
            .iter()
            .zip(&urls)
            .map(|(candidate, url)| CandidateRecord {
                candidate: candidate.clone(),
                url: url.clone(),
            })
            .collect(),
        cases: cases.iter().map(|c| c.id.clone()).collect(),
        targets,
        settings: args.settings.clone(),
        planned_trials: planned,
    };
    evidence::append(&mut file, &path, &Line::Run(manifest.clone()))?;
    eprintln!(
        "planned: {} × {} × {} ({} unlabelled skipped) × {} = {}, up to {}",
        plural(candidates.len(), "candidate", "candidates"),
        plural(instructions.len(), "instruction set", "instruction sets"),
        plural(plan.cases, "case", "cases"),
        plan.skipped,
        plural(
            args.settings.repetitions as usize,
            "repetition",
            "repetitions"
        ),
        plural(planned, "trial", "trials"),
        plural(
            planned * (1 + args.settings.retries as usize),
            "request",
            "requests"
        ),
    );

    let job = Job {
        candidates: &candidates,
        urls: &urls,
        keys: &keys,
        instructions: &instructions,
        cases: &cases,
        targets: &manifest.targets,
    };
    let cancel = Arc::new(AtomicBool::new(false));
    let started = Arc::new(AtomicUsize::new(0));
    let recorded = Arc::new(AtomicUsize::new(0));
    {
        let (cancel, started, recorded) = (cancel.clone(), started.clone(), recorded.clone());
        let signals = AtomicUsize::new(0);
        ctrlc::set_handler(move || {
            if signals.fetch_add(1, Ordering::Relaxed) > 0 {
                std::process::exit(130);
            }
            cancel.store(true, Ordering::Relaxed);
            let in_flight = started
                .load(Ordering::Relaxed)
                .saturating_sub(recorded.load(Ordering::Relaxed));
            eprintln!(
                "cancelling: waiting for {}; press Ctrl-C again to stop now",
                plural(in_flight, "in-flight trial", "in-flight trials")
            );
        })
        .map_err(|e| format!("cannot install the Ctrl-C handler: {e}"))?;
    }
    let (tx, rx) = mpsc::channel();
    let mut trials = Vec::new();
    let mut write_error: Option<String> = None;
    std::thread::scope(|scope| {
        let (job, specs, settings) = (&job, &plan.trials, &args.settings);
        let (cancel, started) = (&*cancel, &*started);
        scope.spawn(move || run::execute(job, specs, settings, cancel, started, tx));
        for trial in rx {
            eprintln!("{}", trial.progress(trials.len() + 1, planned));
            let line = Line::Trial(trial.clone());
            if write_error.is_some() {
                // The file is no longer trustworthy, but the reply is paid for.
                eprint!("{}", evidence::encode(&line));
            } else if let Err(e) = evidence::append(&mut file, &path, &line) {
                write_error = Some(e);
                cancel.store(true, Ordering::Relaxed);
            }
            trials.push(trial);
            recorded.store(trials.len(), Ordering::Relaxed);
        }
    });

    let mut state = if write_error.is_some() {
        RunState::Failed
    } else if trials.len() < planned {
        RunState::Partial
    } else {
        RunState::Complete
    };
    let mut end = EndRecord {
        state,
        finished_unix_ms: now_unix_ms(),
    };
    let end_line = Line::End(end.clone());
    let written = if write_error.is_some() {
        evidence::append_on_new_line(&mut file, &path, &end_line)
    } else {
        evidence::append(&mut file, &path, &end_line)
    };
    if let Err(e) = written {
        write_error.get_or_insert(e);
        state = RunState::Failed;
        end.state = state;
    }
    trials.sort_by_key(|t| t.seq);
    let all_answered = trials.iter().all(|t| t.answered());
    let summary = RunFile {
        manifest,
        trials,
        end: Some(end),
        warnings: Vec::new(),
    };
    print!("{summary}");
    println!("evidence: {}", path.display());
    if let Some(e) = write_error {
        return Err(e);
    }
    Ok(match state {
        RunState::Complete if all_answered => ExitCode::SUCCESS,
        RunState::Complete => ExitCode::from(3),
        RunState::Partial => ExitCode::from(130),
        RunState::Failed => ExitCode::FAILURE,
    })
}

fn load_run(path: &Path) -> Result<RunFile, String> {
    evidence::read_run(&read(path)?)
        .map_err(|e| format!("invalid evidence {}: {e}", path.display()))
}

fn show(path: &Path) -> Result<(), String> {
    print!("{}", report::report(&load_run(path)?));
    Ok(())
}

fn compare(paths: &[PathBuf]) -> Result<(), String> {
    let runs: Vec<RunFile> = paths
        .iter()
        .map(|p| load_run(p))
        .collect::<Result<_, _>>()?;
    for warning in runs.iter().flat_map(|r| &r.warnings) {
        eprintln!("warning: {warning}");
    }
    print!("{}", report::Comparison(&runs));
    Ok(())
}

fn history(dir: Option<&Path>) -> Result<(), String> {
    let dir = match dir {
        Some(dir) => dir.to_path_buf(),
        None => default_path("XDG_DATA_HOME", ".local/share", "runs")?,
    };
    let listing = match std::fs::read_dir(&dir) {
        Ok(listing) => listing,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            println!("no runs in {}", dir.display());
            return Ok(());
        }
        Err(e) => return Err(format!("cannot read {}: {e}", dir.display())),
    };
    let mut entries = Vec::new();
    for entry in listing {
        let path = entry
            .map_err(|e| format!("cannot read {}: {e}", dir.display()))?
            .path();
        if path.extension().is_some_and(|ext| ext == "jsonl") {
            let run = read(&path).and_then(|bytes| evidence::read_run(&bytes));
            entries.push((
                path.file_name().unwrap().to_string_lossy().into_owned(),
                run,
            ));
        }
    }
    if entries.is_empty() {
        println!("no runs in {}", dir.display());
    } else {
        print!("{}", report::history(entries));
    }
    Ok(())
}
