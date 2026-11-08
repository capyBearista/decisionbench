//! Everything `run` does that is not parsing arguments or printing: loading inputs, validating
//! them, writing the evidence file and running the Trials. The CLI and the TUI both call it.

use std::collections::HashSet;
use std::fs::Permissions;
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc;

use toml_edit::{DocumentMut, TomlError, value};

use crate::cost::plural;
use crate::credentials;
use crate::dataset::{BUNDLED_CASES, Case, Split, parse_cases};
use crate::evidence::{
    self, CandidateRecord, EndRecord, GRADING_REVISION, Hashed, InstructionsRecord, Line, Manifest,
    RunFile, RunState, RunSummary, TrialRecord,
};
use crate::grade::Target;
use crate::instructions::{BUNDLED_NAME, BUNDLED_TEXT, JudgeInstructions, parse_instructions};
use crate::provider::{
    Candidate, ConfigFile, env_nonempty, now_unix_ms, parse_config, request_url,
};
use crate::run::{self, Job, Plan, Settings};
use crate::sha256_hex;

pub fn read(path: &Path) -> Result<Vec<u8>, String> {
    std::fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))
}

/// Follows symlinks by hand so a dangling link still resolves to its (missing) target.
pub fn resolve_links(path: &Path) -> Result<PathBuf, String> {
    let mut path = path.to_path_buf();
    for _ in 0..40 {
        let is_link = std::fs::symlink_metadata(&path).is_ok_and(|m| m.is_symlink());
        if !is_link {
            return Ok(path);
        }
        let link = std::fs::read_link(&path)
            .map_err(|e| format!("cannot read link {}: {e}", path.display()))?;
        path = match path.parent() {
            Some(parent) => parent.join(link),
            None => link,
        };
    }
    Err(format!("too many symlinks at {}", path.display()))
}

/// Writes `text` to a new temp file beside `path`, then renames it over `path`: a failed write
/// never leaves a half-written file, and a failed attempt removes its temp file. Every call gets
/// its own temp name (process id and a process-wide counter) and creates it exclusively, so it never
/// reuses or follows a file or symlink already there, and concurrent writes never share one. The
/// temp file is private (0600) before any text goes in; `permissions` are applied to the finished
/// file.
pub fn write_atomic(path: &Path, text: &str, permissions: Permissions) -> std::io::Result<()> {
    static NEXT_TEMP: AtomicUsize = AtomicUsize::new(0);
    let mut temp = path.to_path_buf().into_os_string();
    temp.push(format!(
        ".{}.{}.tmp",
        std::process::id(),
        NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
    ));
    // O_EXCL: if this fails the name is not ours, so nothing is removed.
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temp)?;
    let write = || {
        file.set_permissions(Permissions::from_mode(0o600))?;
        file.write_all(text.as_bytes())?;
        file.set_permissions(permissions)?;
        std::fs::rename(&temp, path)
    };
    let result = write();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result
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

/// The config in `path`: its Candidates and prompt folder.
pub fn load_config(path: &Path) -> Result<ConfigFile, String> {
    let text = String::from_utf8(read(path)?)
        .map_err(|_| format!("invalid config {}: not UTF-8", path.display()))?;
    parse_config(&text).map_err(|e| format!("invalid config {}: {e}", path.display()))
}

/// Rewrites the config at `path` (a missing one reads as `None`) with the text `edit` returns. A
/// symlinked config is written through, so the link survives the rename; an existing config keeps
/// its mode and a new one is private.
pub fn edit_config(
    path: &Path,
    edit: impl FnOnce(Option<&str>) -> Result<String, String>,
) -> Result<(), String> {
    let config = resolve_links(path)?;
    let existing = match std::fs::read_to_string(&config) {
        Ok(text) => Some(text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(format!("cannot read {}: {e}", config.display())),
    };
    let text = edit(existing.as_deref())?;
    if let Some(dir) = config.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    }
    let permissions = std::fs::metadata(&config)
        .map(|m| m.permissions())
        .unwrap_or_else(|_| Permissions::from_mode(0o600));
    write_atomic(&config, &text, permissions)
        .map_err(|e| format!("cannot write {}: {e}", path.display()))
}

/// Sets (non-blank `dir`) or removes (`None` or blank) the config's `prompts_dir`, leaving the rest
/// of the file as it is.
pub fn set_prompts_dir(config: &Path, dir: Option<&str>) -> Result<(), String> {
    edit_config(config, |existing| {
        let mut doc: DocumentMut = existing
            .unwrap_or_default()
            .parse()
            .map_err(|e: TomlError| e.to_string())?;
        match dir.map(str::trim).filter(|dir| !dir.is_empty()) {
            Some(dir) => doc["prompts_dir"] = value(dir),
            None => {
                doc.remove("prompts_dir");
            }
        }
        let text = doc.to_string();
        parse_config(&text)?;
        Ok(text)
    })
}

/// The prompt folder `dir` names: a relative one is beside the config file, an absolute one as is.
pub fn prompts_dir_path(config: &Path, dir: &str) -> PathBuf {
    config.parent().unwrap_or(Path::new("")).join(dir)
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
pub type RunEntry = (String, Result<RunSummary, String>);

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
            let run = read(&path).and_then(|bytes| evidence::read_summary(&bytes));
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
    /// `None` is the bundled `neutral`, `Some` an instructions file.
    pub instructions: Vec<Option<PathBuf>>,
    pub cases: Vec<String>,
    /// Keep only the Cases in this part of the dataset.
    pub split: Option<Split>,
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
    let configured = load_config(&config)?.candidates;
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

    let instructions: Vec<JudgeInstructions> = args
        .instructions
        .iter()
        .map(|path| load_instructions(path.as_deref()))
        .collect::<Result<_, _>>()?;
    reject_duplicates(
        "instructions name",
        instructions.iter().map(|i| i.name.clone()),
    )?;

    let (dataset_name, dataset_sha256, mut cases) = load_dataset(args.dataset.as_deref())?;
    if let Some(split) = args.split {
        cases.retain(|c| c.split == split);
    }
    reject_duplicates("case", args.cases.iter().cloned())?;
    if let Some(id) = args
        .cases
        .iter()
        .find(|id| !cases.iter().any(|c| &c.id == *id))
    {
        let part = args
            .split
            .map_or(String::new(), |s| format!(" (split {})", s.name()));
        return Err(format!("unknown case {id} in {dataset_name}{part}"));
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

    let credentials = credentials::path_for(&config);
    let keys: Vec<Option<String>> = candidates
        .iter()
        .map(|c| c.provider.key(&credentials))
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
                run::request_count(planned, settings.retries),
                "request",
                "requests"
            ),
        )
    }

    /// Runs the Trials; the calling thread is the only writer of the evidence file. Workers send
    /// finished Trials over a channel and each becomes one line. `on_trial` sees every Trial after it
    /// has been offered to the file, with its 1-based position and whether it is unsaved: this write or an
    /// earlier one failed, so the caller must keep the Trial itself (see `evidence::printable`).
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
                let line = Line::Trial(trial.clone());
                // The file is no longer trustworthy once a write failed, but the reply is paid for.
                if write_error.is_none()
                    && let Err(e) = evidence::append(&mut file, &path, &line)
                {
                    write_error = Some(e);
                    cancel.store(true, Ordering::Relaxed);
                }
                on_trial(&trial, trials.len() + 1, write_error.is_some());
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

/// Names in `dir` that end in `.tmp`, sorted: what a write should not leave behind.
#[cfg(test)]
pub fn temp_files_in(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".tmp"))
        .collect();
    names.sort();
    names
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::time::Duration;

    static NEXT_DIR: AtomicUsize = AtomicUsize::new(0);

    /// A fresh, empty directory of its own.
    fn fresh_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "decisionbench-atomic-{}-{}",
            std::process::id(),
            NEXT_DIR.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn mode(path: &Path) -> u32 {
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn a_failed_atomic_write_removes_its_temp_file() {
        let dir = fresh_dir();
        // The target is a directory, so the final rename fails after the temp file was written.
        std::fs::create_dir(dir.join("target")).unwrap();
        let result = write_atomic(&dir.join("target"), "secret", Permissions::from_mode(0o600));
        assert!(result.is_err());
        assert_eq!(temp_files_in(&dir), Vec::<String>::new());
    }

    #[test]
    fn consecutive_atomic_writes_leave_no_temp_file() {
        let dir = fresh_dir();
        let target = dir.join("target");
        for text in ["one", "two"] {
            write_atomic(&target, text, Permissions::from_mode(0o640)).unwrap();
            assert_eq!(std::fs::read_to_string(&target).unwrap(), text);
            assert_eq!(mode(&target), 0o640);
            assert_eq!(temp_files_in(&dir), Vec::<String>::new());
        }
    }

    /// A symlink or stale file at the old fixed temp name is neither followed nor reused.
    #[test]
    fn an_atomic_write_ignores_a_planted_symlink_and_a_stale_world_readable_temp_file() {
        let dir = fresh_dir();
        let victim = dir.join("victim");
        std::os::unix::fs::symlink(&victim, dir.join("linked.tmp")).unwrap();
        std::fs::write(dir.join("stale.tmp"), "stale").unwrap();
        std::fs::set_permissions(dir.join("stale.tmp"), Permissions::from_mode(0o644)).unwrap();
        for name in ["linked", "stale"] {
            write_atomic(&dir.join(name), "secret", Permissions::from_mode(0o600)).unwrap();
            assert_eq!(std::fs::read_to_string(dir.join(name)).unwrap(), "secret");
            assert_eq!(mode(&dir.join(name)), 0o600);
        }
        assert!(!victim.exists());
        assert!(
            std::fs::symlink_metadata(dir.join("linked.tmp"))
                .unwrap()
                .is_symlink()
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("stale.tmp")).unwrap(),
            "stale"
        );
        assert_eq!(mode(&dir.join("stale.tmp")), 0o644);
        // Only the two planted files are left.
        assert_eq!(temp_files_in(&dir), ["linked.tmp", "stale.tmp"]);
    }

    /// Writers racing on one target never share a temp file, so none of them fails.
    #[test]
    fn concurrent_atomic_writes_to_one_target_never_fail_and_leave_one_file() {
        let dir = fresh_dir();
        let target = dir.join("target");
        std::thread::scope(|scope| {
            for text in ["first", "second"] {
                let target = &target;
                scope.spawn(move || {
                    for _ in 0..50 {
                        write_atomic(target, text, Permissions::from_mode(0o600)).unwrap();
                    }
                });
            }
        });
        let saved = std::fs::read_to_string(&target).unwrap();
        assert!(saved == "first" || saved == "second", "{saved}");
        assert_eq!(temp_files_in(&dir), Vec::<String>::new());
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
    }

    const CONFIG: &str =
        "# my config\n\n[[candidate]]\nname = \"a\"\nprovider = \"zen\"\nmodel = \"m\" # pinned\n";

    fn write_config_file(dir: &Path, text: &str) -> PathBuf {
        let config = dir.join("config.toml");
        std::fs::write(&config, text).unwrap();
        config
    }

    fn run_args(dir: &Path, instructions: Vec<Option<PathBuf>>) -> RunArgs {
        RunArgs {
            candidates: vec!["a".into()],
            instructions,
            cases: ["c001", "c002", "c003"].map(String::from).to_vec(),
            split: None,
            targets: vec![Target::Sonnet],
            settings: Settings {
                repetitions: 1,
                concurrency: 3,
                timeout_secs: 5,
                retries: 0,
                include_unscored: true,
            },
            config: Some(write_config_file(dir, CONFIG)),
            dataset: None,
            out: Some(dir.join("runs")),
        }
    }

    /// A prompt file named `name` that holds the bundled instructions.
    fn prompt_file(dir: &Path, name: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, BUNDLED_TEXT).unwrap();
        path
    }

    #[test]
    fn prepare_mixes_the_bundled_instructions_with_files() {
        let dir = fresh_dir();
        let file = prompt_file(&dir, "mine.md");
        let prepared = prepare(&run_args(&dir, vec![None, Some(file)])).unwrap();
        let names: Vec<_> = prepared.instructions.iter().map(|i| &i.name[..]).collect();
        assert_eq!(names, ["neutral", "mine"]);
        assert_eq!(prepared.planned(), 2 * 3);
    }

    #[test]
    fn a_file_named_like_the_bundled_instructions_is_a_duplicate() {
        let dir = fresh_dir();
        let file = prompt_file(&dir, "neutral.md");
        let err = prepare(&run_args(&dir, vec![None, Some(file)]))
            .err()
            .unwrap();
        assert_eq!(err, "duplicate instructions name `neutral`");
        assert!(!dir.join("runs").exists(), "no evidence file was created");
    }

    /// A write error mid-run (the evidence file is /dev/full, which refuses every write) fails the
    /// Run, and every Trial from the failing one on is handed to the caller as unsaved.
    #[test]
    fn a_failed_evidence_write_fails_the_run_and_flags_every_trial_unsaved() {
        let dir = fresh_dir();
        let mut prepared = prepare(&run_args(&dir, vec![None])).unwrap();
        // A server that holds all three requests until every Trial is in flight, then hangs up, so
        // the first write failure finds the other two Trials already started.
        let server = TcpListener::bind("127.0.0.1:0").unwrap();
        prepared.urls = vec![format!("http://{}", server.local_addr().unwrap())];
        let hold = std::thread::spawn(move || {
            let held: Vec<_> = (0..3).map(|_| server.accept().unwrap()).collect();
            std::thread::sleep(Duration::from_millis(300));
            drop(held);
        });
        prepared.file = std::fs::OpenOptions::new()
            .write(true)
            .open("/dev/full")
            .unwrap();

        let mut unsaved = Vec::new();
        let outcome = prepared.run(&Control::default(), |_, _, lost| unsaved.push(lost));
        hold.join().unwrap();

        assert_eq!(outcome.state, RunState::Failed);
        assert!(outcome.write_error.is_some());
        assert_eq!(unsaved, [true, true, true]);
    }

    #[test]
    fn set_prompts_dir_sets_replaces_and_clears_only_that_key() {
        let dir = fresh_dir();
        let config = write_config_file(&dir, CONFIG);
        let read = || std::fs::read_to_string(&config).unwrap();
        let folder = || load_config(&config).unwrap().prompts_dir;

        set_prompts_dir(&config, Some("  mine ")).unwrap();
        assert_eq!(folder().as_deref(), Some("mine"));
        assert!(read().contains("prompts_dir = \"mine\""), "{}", read());
        for kept in ["# my config", "model = \"m\" # pinned", "name = \"a\""] {
            assert!(read().contains(kept), "{kept} missing from:\n{}", read());
        }
        assert_eq!(load_config(&config).unwrap().candidates.len(), 1);

        set_prompts_dir(&config, Some("other")).unwrap();
        assert_eq!(folder().as_deref(), Some("other"));
        assert_eq!(read().matches("prompts_dir").count(), 1);

        // Blank clears like `None`, and the file is what it was before the key came.
        set_prompts_dir(&config, Some("   ")).unwrap();
        assert_eq!(folder(), None);
        assert_eq!(read(), CONFIG);
        set_prompts_dir(&config, None).unwrap();
        assert_eq!(read(), CONFIG);
        assert_eq!(temp_files_in(&dir), Vec::<String>::new());
    }

    #[test]
    fn set_prompts_dir_creates_a_missing_config_privately_and_keeps_an_existing_mode() {
        let dir = fresh_dir();
        let created = dir.join("deeper/config.toml");
        set_prompts_dir(&created, Some("p")).unwrap();
        assert_eq!(
            load_config(&created).unwrap().prompts_dir.as_deref(),
            Some("p")
        );
        assert_eq!(mode(&created), 0o600);

        let shared = write_config_file(&dir, CONFIG);
        std::fs::set_permissions(&shared, Permissions::from_mode(0o640)).unwrap();
        set_prompts_dir(&shared, Some("p")).unwrap();
        assert_eq!(mode(&shared), 0o640);
    }

    #[test]
    fn set_prompts_dir_writes_through_a_symlinked_config() {
        let dir = fresh_dir();
        std::fs::create_dir(dir.join("real")).unwrap();
        let target = write_config_file(&dir.join("real"), CONFIG);
        let link = dir.join("link.toml");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        set_prompts_dir(&link, Some("p")).unwrap();
        assert!(std::fs::symlink_metadata(&link).unwrap().is_symlink());
        assert_eq!(
            load_config(&target).unwrap().prompts_dir.as_deref(),
            Some("p")
        );
    }

    #[test]
    fn set_prompts_dir_refuses_an_invalid_config_and_leaves_it_alone() {
        let dir = fresh_dir();
        for bad in ["not [toml", "stray = 1\n"] {
            let config = write_config_file(&dir, bad);
            let err = set_prompts_dir(&config, Some("p")).unwrap_err();
            assert!(!err.is_empty());
            assert_eq!(std::fs::read_to_string(&config).unwrap(), bad);
            assert_eq!(temp_files_in(&dir), Vec::<String>::new());
        }
    }

    #[test]
    fn prompts_dir_path_resolves_a_relative_folder_beside_the_config() {
        let config = Path::new("/etc/bench/config.toml");
        assert_eq!(
            prompts_dir_path(config, "prompts"),
            Path::new("/etc/bench/prompts")
        );
        assert_eq!(
            prompts_dir_path(config, "/srv/prompts"),
            Path::new("/srv/prompts")
        );
        assert_eq!(
            prompts_dir_path(Path::new("config.toml"), "p"),
            Path::new("p")
        );
    }
}
