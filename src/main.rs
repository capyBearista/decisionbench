use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use decisionbench::dataset::{BUNDLED_CASES, Case, parse_cases, summarize};
use decisionbench::evidence::{self, Evidence, GRADING_REVISION, Hashed, InstructionsRecord};
use decisionbench::grade::{Tally, Target, grade_all, parse_predictions};
use decisionbench::instructions::{
    BUNDLED_NAME, BUNDLED_TEXT, JudgeInstructions, parse_instructions,
};
use decisionbench::provider::{
    Candidate, Disposition, env_nonempty, normalize, now_unix_ms, parse_candidates, request_body,
    request_url, send,
};
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
    /// Send one Decisions request for a Case and Target Model, save the evidence and grade it.
    ///
    /// Exits 3 when the provider or transport failed (evidence is still saved).
    Evaluate {
        /// Candidate name from the config file.
        #[arg(long)]
        candidate: String,
        /// Case id in the dataset.
        #[arg(long = "case")]
        case_id: String,
        #[arg(long, value_enum)]
        target: Target,
        /// Judge Instructions file; defaults to the bundled `neutral`.
        #[arg(long)]
        instructions: Option<PathBuf>,
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
    /// Print the summary and grade of a saved evidence file (offline).
    Show { file: PathBuf },
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
        Command::Evaluate {
            candidate,
            case_id,
            target,
            instructions,
            config,
            dataset,
            out,
        } => evaluate(&Evaluation {
            candidate,
            case_id,
            target,
            instructions,
            config,
            dataset,
            out,
        }),
        Command::Show { file } => show(&file).map(|()| ExitCode::SUCCESS),
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

struct Evaluation {
    candidate: String,
    case_id: String,
    target: Target,
    instructions: Option<PathBuf>,
    config: Option<PathBuf>,
    dataset: Option<PathBuf>,
    out: Option<PathBuf>,
}

/// Every input is checked, and the evidence file is created, before the request, so an input or
/// output error sends nothing. If the final write fails the evidence JSON goes to stderr instead.
fn evaluate(args: &Evaluation) -> Result<ExitCode, String> {
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
    let candidate: Candidate = parse_candidates(&text)
        .map_err(|e| format!("invalid config {}: {e}", config.display()))?
        .into_iter()
        .find(|c| c.name == args.candidate)
        .ok_or_else(|| {
            format!(
                "unknown candidate `{}` in {}",
                args.candidate,
                config.display()
            )
        })?;
    let instructions = load_instructions(args.instructions.as_deref())?;
    let (dataset_name, dataset_sha256, cases) = load_dataset(args.dataset.as_deref())?;
    let case = cases
        .iter()
        .find(|c| c.id == args.case_id)
        .ok_or_else(|| format!("unknown case {} in {dataset_name}", args.case_id))?;
    let key = candidate.provider.key()?;

    let id = format!("{}-{}", now_unix_ms(), std::process::id());
    let (file, path) = evidence::create(&out, &id)?;
    let request = request_body(&candidate.model, &instructions, case, args.target);
    let url = request_url(candidate.provider);
    let attempt = send(&url, key.as_deref(), &request);
    let created_unix_ms = now_unix_ms();
    let prediction = (attempt.disposition == Disposition::Answered)
        .then(|| normalize(attempt.raw_response.as_deref().unwrap_or_default()));
    let evidence = Evidence {
        schema_version: evidence::SCHEMA_VERSION,
        id,
        created_unix_ms,
        dataset: Hashed {
            name: dataset_name,
            sha256: dataset_sha256,
        },
        instructions: InstructionsRecord {
            name: instructions.name,
            sha256: instructions.sha256,
            text: instructions.text,
        },
        grading_revision: GRADING_REVISION,
        candidate,
        url,
        case_id: case.id.clone(),
        target: args.target,
        label: match args.target {
            Target::Sonnet => case.ok.sonnet,
            Target::Opus => case.ok.opus,
        },
        request,
        attempt,
        prediction,
    };
    evidence::save(file, &path, &evidence)?;
    print!("{evidence}");
    println!("evidence: {}", path.display());
    Ok(if evidence.prediction.is_some() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(3)
    })
}

fn show(path: &Path) -> Result<(), String> {
    let evidence = evidence::read(&read(path)?)
        .map_err(|e| format!("invalid evidence {}: {e}", path.display()))?;
    print!("{evidence}");
    Ok(())
}
