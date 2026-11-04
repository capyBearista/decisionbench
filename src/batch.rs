//! Everything `run` does that is not parsing arguments or printing: loading inputs, validating
//! them, writing the evidence file and running the Trials. The CLI and the TUI both call it.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc;

use crate::cost::plural;
use crate::dataset::{BUNDLED_CASES, Case, parse_cases};
use crate::evidence::{
    self, CandidateRecord, EndRecord, GRADING_REVISION, Hashed, InstructionsRecord, Line, Manifest,
    RunFile, RunState, TrialRecord,
};
use crate::grade::Target;
use crate::instructions::{BUNDLED_NAME, BUNDLED_TEXT, JudgeInstructions, parse_instructions};
use crate::provider::{Candidate, env_nonempty, now_unix_ms, parse_candidates, request_url};
use crate::run::{self, Job, Plan, Settings};
use crate::sha256_hex;

pub fn read(path: &Path) -> Result<Vec<u8>, String> {
    std::fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))
}

/// The dataset's display name, SHA-256 of its bytes (its revision) and Cases.
pub fn load_dataset(path: Option<&Path>) -> Result<(String, String, Vec<Case>), String> {
    let (name, bytes) = match path {
        None => ("bundled cases".to_string(), BUNDLED_CASES.to_vec()),
        Some(p) => (p.display().to_string(), read(p)?),
    };
    let cases = parse_cases(&bytes).map_err(|e| format!("invalid dataset {name}: {e}"))?;
    Ok((name, sha256_hex(&bytes), cases))
}

/// `$XDG_*` if set, else `$HOME/<home_rel>`, then `decisionbench/<rest>`.
fn default_path(xdg_var: &str, home_rel: &str, rest: &str) -> Result<PathBuf, String> {
    env_nonempty(xdg_var)
        .map(PathBuf::from)
        .or_else(|| env_nonempty("HOME").map(|home| Path::new(&home).join(home_rel)))
        .map(|base| base.join("decisionbench").join(rest))
        .ok_or_else(|| format!("cannot find ${xdg_var} or $HOME; pass the path explicitly"))
}

/// The given config file, else `$XDG_CONFIG_HOME/decisionbench/config.toml`.
pub fn config_path(given: Option<&Path>) -> Result<PathBuf, String> {
    match given {
        Some(path) => Ok(path.to_path_buf()),
        None => default_path("XDG_CONFIG_HOME", ".config", "config.toml"),
    }
}

/// The given evidence directory, else `$XDG_DATA_HOME/decisionbench/runs`.
pub fn runs_dir(given: Option<&Path>) -> Result<PathBuf, String> {
    match given {
        Some(path) => Ok(path.to_path_buf()),
        None => default_path("XDG_DATA_HOME", ".local/share", "runs"),
    }
}

/// The Candidates configured in `path`.
pub fn load_config(path: &Path) -> Result<Vec<Candidate>, String> {
    let text = String::from_utf8(read(path)?)
        .map_err(|_| format!("invalid config {}: not UTF-8", path.display()))?;
    parse_candidates(&text).map_err(|e| format!("invalid config {}: {e}", path.display()))
}

pub fn load_instructions(path: Option<&Path>) -> Result<JudgeInstructions, String> {
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

pub fn load_run(path: &Path) -> Result<RunFile, String> {
    evidence::read_run(&read(path)?)
        .map_err(|e| format!("invalid evidence {}: {e}", path.display()))
}

/// A file name and its parsed Run or read error.
pub type RunEntry = (String, Result<RunFile, String>);

/// Every `.jsonl` file in `dir`; empty when `dir` does not exist.
pub fn read_runs(dir: &Path) -> Result<Vec<RunEntry>, String> {
    let listing = match std::fs::read_dir(dir) {
        Ok(listing) => listing,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
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
    Ok(entries)
}

fn reject_duplicates(kind: &str, items: impl IntoIterator<Item = String>) -> Result<(), String> {
    let mut seen = HashSet::new();
    match items.into_iter().find(|item| !seen.insert(item.clone())) {
        Some(item) => Err(format!("duplicate {kind} `{item}`")),
        None => Ok(()),
    }
}

pub struct RunArgs {
    pub candidates: Vec<String>,
    pub instructions: Vec<PathBuf>,
    pub cases: Vec<String>,
    pub targets: Vec<Target>,
    pub settings: Settings,
    pub config: Option<PathBuf>,
    pub dataset: Option<PathBuf>,
    pub out: Option<PathBuf>,
}

/// Shared between the thread running the Trials and whoever may cancel them.
#[derive(Default)]
pub struct Control {
    pub cancel: AtomicBool,
    /// Trials taken by a worker.
    pub started: AtomicUsize,
    /// Trials written to the evidence file.
    pub recorded: AtomicUsize,
}

impl Control {
    pub fn in_flight(&self) -> usize {
        self.started
            .load(Ordering::Relaxed)
            .saturating_sub(self.recorded.load(Ordering::Relaxed))
    }
}

/// A validated Run whose evidence file exists and holds the manifest; nothing is sent yet.
pub struct Prepared {
    candidates: Vec<Candidate>,
    urls: Vec<String>,
    keys: Vec<Option<String>>,
    instructions: Vec<JudgeInstructions>,
    cases: Vec<Case>,
    plan: Plan,
    manifest: Manifest,
    file: std::fs::File,
    pub path: PathBuf,
}

/// Every input is checked, and the evidence file is created, before the first request, so an input
/// or output error sends nothing.
pub fn prepare(args: &RunArgs) -> Result<Prepared, String> {
    let config = config_path(args.config.as_deref())?;
    let out = runs_dir(args.out.as_deref())?;
    let configured = load_config(&config)?;
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
        planned_trials: plan.trials.len(),
    };
    evidence::append(&mut file, &path, &Line::Run(manifest.clone()))?;
    Ok(Prepared {
        candidates,
        urls,
        keys,
        instructions,
        cases,
        plan,
        manifest,
        file,
        path,
    })
}

/// What a finished `Prepared::run` leaves behind.
pub struct Outcome {
    pub summary: RunFile,
    pub state: RunState,
    pub all_answered: bool,
    pub write_error: Option<String>,
}

impl Prepared {
    pub fn planned(&self) -> usize {
        self.plan.trials.len()
    }

    /// The one-line plan the CLI prints before the first request.
    pub fn plan_line(&self) -> String {
        let settings = &self.manifest.settings;
        let planned = self.planned();
        format!(
            "planned: {} × {} × {} ({} unlabelled skipped) × {} = {}, up to {}",
            plural(self.candidates.len(), "candidate", "candidates"),
            plural(
                self.instructions.len(),
                "instruction set",
                "instruction sets"
            ),
            plural(self.plan.cases, "case", "cases"),
            self.plan.skipped,
            plural(settings.repetitions as usize, "repetition", "repetitions"),
            plural(planned, "trial", "trials"),
            plural(
                planned * (1 + settings.retries as usize),
                "request",
                "requests"
            ),
        )
    }

    /// Runs the Trials; the calling thread is the only writer of the evidence file. Workers send
    /// finished Trials over a channel and each becomes one line. `on_trial` sees every Trial before it
    /// is written, its 1-based position and whether an earlier write failed (the caller must then
    /// print the Trial's line itself, since the file no longer takes it).
    pub fn run(
        self,
        control: &Control,
        mut on_trial: impl FnMut(&TrialRecord, usize, bool),
    ) -> Outcome {
        let Self {
            candidates,
            urls,
            keys,
            instructions,
            cases,
            plan,
            manifest,
            mut file,
            path,
        } = self;
        let planned = plan.trials.len();
        let job = Job {
            candidates: &candidates,
            urls: &urls,
            keys: &keys,
            instructions: &instructions,
            cases: &cases,
            targets: &manifest.targets,
        };
        let (tx, rx) = mpsc::channel();
        let mut trials = Vec::new();
        let mut write_error: Option<String> = None;
        std::thread::scope(|scope| {
            let (job, specs, settings) = (&job, &plan.trials, &manifest.settings);
            let (cancel, started) = (&control.cancel, &control.started);
            scope.spawn(move || run::execute(job, specs, settings, cancel, started, tx));
            for trial in rx {
                on_trial(&trial, trials.len() + 1, write_error.is_some());
                let line = Line::Trial(trial.clone());
                // The file is no longer trustworthy once a write failed, but the reply is paid for.
                if write_error.is_none()
                    && let Err(e) = evidence::append(&mut file, &path, &line)
                {
                    write_error = Some(e);
                    cancel.store(true, Ordering::Relaxed);
                }
                trials.push(trial);
                control.recorded.store(trials.len(), Ordering::Relaxed);
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
        Outcome {
            summary: RunFile {
                manifest,
                trials,
                end: Some(end),
                warnings: Vec::new(),
            },
            state,
            all_answered,
            write_error,
        }
    }
}
