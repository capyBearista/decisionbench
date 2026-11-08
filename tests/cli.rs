use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};

const EXPECTED_COUNTS: &str = "\
cases: 66
haiku labels: 66 (0 unscored)
sonnet labels: 66 (0 unscored)
opus labels: 66 (0 unscored)
fable labels: 66 (0 unscored)
";

fn run(args: &[&str]) -> Output {
    run_with_env(args, &[])
}

/// Keys and proxies are cleared first so the host environment cannot leak into a test.
fn run_with_env(args: &[&str], envs: &[(&str, &str)]) -> Output {
    command_with_env(args, envs).output().expect("binary runs")
}

fn command_with_env(args: &[&str], envs: &[(&str, &str)]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_decisionbench"));
    for name in [
        "OPENROUTER_API_KEY",
        "OPENCODE_API_KEY",
        "DECISIONBENCH_TEST_BASE_URL",
        "HTTP_PROXY",
        "http_proxy",
        "HTTPS_PROXY",
        "https_proxy",
        "ALL_PROXY",
        "all_proxy",
    ] {
        command.env_remove(name);
    }
    command.args(args).envs(envs.iter().copied());
    command
}

fn dataset_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("datasets/cases.json")
}

/// Writes `contents` to a uniquely named file in the target temp dir.
fn temp_file(name: &str, contents: &str) -> PathBuf {
    let path = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    std::fs::write(&path, contents).unwrap();
    path
}

fn stdout_of_success(out: Output) -> String {
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

#[test]
fn inspect_bundled_dataset_prints_summary() {
    let stdout = stdout_of_success(run(&["dataset", "inspect"]));
    assert_eq!(stdout, format!("dataset: bundled cases\n{EXPECTED_COUNTS}"));
}

#[test]
fn inspect_explicit_path_prints_same_counts() {
    let path = dataset_path();
    let stdout = stdout_of_success(run(&["dataset", "inspect", path.to_str().unwrap()]));
    assert_eq!(
        stdout,
        format!("dataset: {}\n{EXPECTED_COUNTS}", path.display())
    );
}

const VALID: &str = r#"{"id":"c001","message":"m","ok":{"sonnet":"low"}}"#;

fn dataset(cases: &str) -> String {
    format!("[{cases}]")
}

fn inspect_file(name: &str, contents: &str) -> Output {
    let path = temp_file(name, contents);
    run(&["dataset", "inspect", path.to_str().unwrap()])
}

fn assert_accepted(name: &str, contents: &str) -> String {
    stdout_of_success(inspect_file(name, contents))
}

fn assert_rejected(name: &str, contents: &str, expected_fragment: &str) {
    let out = inspect_file(name, contents);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "stderr: {stderr}");
    assert!(out.stdout.is_empty(), "no summary on failure");
    assert!(
        stderr.contains(expected_fragment),
        "expected {expected_fragment:?} in stderr: {stderr}"
    );
}

#[test]
fn accepts_single_levels_ranges_and_partial_labels() {
    let cases = [
        VALID,
        r#"{"id":"c002","message":"n","context":"user: hi","ok":{"sonnet":"max","opus":"low-max"}}"#,
        r#"{"id":"c003","message":"o","ok":{"opus":"medium-xhigh"}}"#,
        r#"{"id":"c004","message":"p","ok":{"haiku":"low","fable":"max"}}"#,
    ];
    let stdout = assert_accepted("valid.json", &dataset(&cases.join(",")));
    assert!(
        stdout.ends_with(
            "cases: 4\nhaiku labels: 1 (3 unscored)\nsonnet labels: 2 (2 unscored)\nopus labels: 2 (2 unscored)\nfable labels: 1 (3 unscored)\n"
        ),
        "{stdout}"
    );
}

#[test]
fn rejects_unknown_effort() {
    let bad = VALID.replace(r#""sonnet":"low""#, r#""sonnet":"huge""#);
    assert_rejected("effort.json", &dataset(&bad), "unknown effort `huge`");
}

#[test]
fn rejects_reversed_range() {
    let bad = VALID.replace(r#""sonnet":"low""#, r#""sonnet":"high-low""#);
    assert_rejected(
        "reversed.json",
        &dataset(&bad),
        "range `high-low` starts above its end",
    );
}

#[test]
fn rejects_malformed_ranges() {
    for (name, range, fragment) in [
        ("trailing.json", "low-", "unknown effort ``"),
        (
            "three.json",
            "low-medium-high",
            "unknown effort `medium-high`",
        ),
    ] {
        let bad = VALID.replace(r#""sonnet":"low""#, &format!(r#""sonnet":"{range}""#));
        assert_rejected(name, &dataset(&bad), fragment);
    }
}

#[test]
fn rejects_unknown_target_model() {
    let bad = VALID.replace(r#""sonnet":"low""#, r#""gemini":"low""#);
    assert_rejected("target.json", &dataset(&bad), "unknown field `gemini`");
}

#[test]
fn rejects_duplicate_target_model() {
    let bad = VALID.replace(r#""sonnet":"low""#, r#""sonnet":"low","sonnet":"high""#);
    assert_rejected(
        "dup-target.json",
        &dataset(&bad),
        "duplicate field `sonnet`",
    );
}

#[test]
fn rejects_session_state_and_unknown_fields() {
    for field in [
        r#""current":{"model":"sonnet","effort":"low"}"#,
        r#""kind":"trivial""#,
    ] {
        let bad = VALID.replace(r#""id":"c001""#, &format!(r#""id":"c001",{field}"#));
        assert_rejected("unknown.json", &dataset(&bad), "unknown field");
    }
}

#[test]
fn rejects_missing_message() {
    let bad = VALID.replace(r#","message":"m""#, "");
    assert_rejected("missing.json", &dataset(&bad), "missing field `message`");
}

#[test]
fn rejects_duplicate_ids() {
    let second = VALID.replace(r#""message":"m""#, r#""message":"other""#);
    assert_rejected(
        "dup-id.json",
        &dataset(&format!("{VALID},{second}")),
        "duplicate case id c001",
    );
}

#[test]
fn rejects_duplicate_message_and_context() {
    let second = VALID.replace("c001", "c002");
    assert_rejected(
        "dup-message.json",
        &dataset(&format!("{VALID},{second}")),
        "case c002 repeats the message and context of c001",
    );
}

#[test]
fn same_message_with_different_context_is_allowed() {
    let second = VALID.replace("c001", "c002").replace(
        r#""message":"m""#,
        r#""message":"m","context":"user: earlier""#,
    );
    assert_accepted("context.json", &dataset(&format!("{VALID},{second}")));
}

#[test]
fn rejects_empty_dataset() {
    assert_rejected("empty.json", "[]", "no cases");
}

#[test]
fn rejects_malformed_json() {
    assert_rejected("malformed.json", "[{", "EOF");
}

#[test]
fn rejects_missing_file() {
    let out = run(&["dataset", "inspect", "/nonexistent/decisionbench.json"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&out.stderr)
            .contains("cannot read /nonexistent/decisionbench.json")
    );
}

#[test]
fn usage_error_exits_2() {
    let out = run(&["dataset", "inspect", "a.json", "b.json"]);
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn rejects_other_malformed_range_text() {
    for (n, label) in [r#""""#, r#""-low""#, r#"" low""#, r#""LOW""#, "5"]
        .iter()
        .enumerate()
    {
        let bad = VALID.replace(r#""sonnet":"low""#, &format!(r#""sonnet":{label}"#));
        let out = inspect_file(&format!("range-{n}.json"), &dataset(&bad));
        assert_eq!(out.status.code(), Some(1), "{label} should be rejected");
    }
}

#[test]
fn accepts_range_with_equal_ends() {
    let good = VALID.replace(r#""sonnet":"low""#, r#""sonnet":"low-low""#);
    assert_accepted("equal-ends.json", &dataset(&good));
}

#[test]
fn rejects_case_without_labels() {
    let bad = VALID.replace(r#"{"sonnet":"low"}"#, "{}");
    assert_rejected("no-labels.json", &dataset(&bad), "case c001 has no labels");
}

#[test]
fn rejects_empty_id() {
    let bad = VALID.replace(r#""id":"c001""#, r#""id":"""#);
    assert_rejected("empty-id.json", &dataset(&bad), "case with empty id");
}

#[test]
fn rejects_null_target_label() {
    assert_rejected(
        "null_label.json",
        &dataset(r#"{"id":"c001","message":"m","ok":{"sonnet":"low","opus":null}}"#),
        "`null` label; omit the key to leave the target unscored",
    );
    assert_rejected(
        "null_sonnet.json",
        &dataset(r#"{"id":"c001","message":"m","ok":{"sonnet":null,"opus":"low"}}"#),
        "`null` label; omit the key to leave the target unscored",
    );
}

/// `name` keeps temp files unique per test, since tests run in parallel.
fn grade_files(name: &str, predictions: &str, dataset: Option<&str>) -> Output {
    let predictions = temp_file(&format!("{name}-predictions.json"), predictions);
    let mut args = vec!["grade".to_string(), predictions.display().to_string()];
    if let Some(cases) = dataset {
        let path = temp_file(&format!("{name}-dataset.json"), cases);
        args.extend(["--dataset".to_string(), path.display().to_string()]);
    }
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    run(&args)
}

fn assert_grade_rejected(predictions: &str, expected_fragment: &str) {
    let out = grade_files("bad", predictions, None);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "stderr: {stderr}");
    assert!(out.stdout.is_empty(), "no grades on failure");
    assert!(
        stderr.contains(expected_fragment),
        "expected {expected_fragment:?} in stderr: {stderr}"
    );
}

#[test]
fn grades_predictions_with_per_target_denominators() {
    let cases = dataset(
        r#"{"id":"c001","message":"m","ok":{"sonnet":"medium-high"}},
           {"id":"c002","message":"n","ok":{"opus":"max"}}"#,
    );
    let predictions = r#"[
        {"case":"c001","target":"sonnet","effort":"high"},
        {"case":"c001","target":"sonnet","effort":"low"},
        {"case":"c001","target":"sonnet","effort":"max"},
        {"case":"c001","target":"sonnet","effort":"High"},
        {"case":"c001","target":"sonnet","effort":"medium"},
        {"case":"c001","target":"opus","effort":"medium"},
        {"case":"c001","target":"opus","effort":"High"},
        {"case":"c002","target":"opus","effort":"max"},
        {"case":"c002","target":"opus"},
        {"case":"c002","target":"opus","effort":null},
        {"case":"c002","target":"opus","effort":7}
    ]"#;
    let stdout = stdout_of_success(grade_files("denominators", predictions, Some(&cases)));
    let expected = "\
c001 sonnet \"high\" hit
c001 sonnet \"low\" under
c001 sonnet \"max\" over
c001 sonnet \"High\" invalid
c001 sonnet \"medium\" hit
c001 opus \"medium\" unscored
c001 opus \"High\" unscored
c002 opus \"max\" hit
c002 opus null invalid
c002 opus null invalid
c002 opus 7 invalid
haiku: 0 predictions: 0 hit, 0 under, 0 over, 0 invalid, 0 unscored; accuracy 0/0 valid scored
sonnet: 5 predictions: 2 hit, 1 under, 1 over, 1 invalid, 0 unscored; accuracy 2/4 valid scored
opus: 6 predictions: 1 hit, 0 under, 0 over, 3 invalid, 2 unscored; accuracy 1/1 valid scored
fable: 0 predictions: 0 hit, 0 under, 0 over, 0 invalid, 0 unscored; accuracy 0/0 valid scored
";
    assert!(stdout.ends_with(expected), "{stdout}");
}

#[test]
fn grades_against_bundled_dataset_by_default() {
    let stdout = stdout_of_success(grade_files(
        "bundled",
        r#"[{"case":"c001","target":"sonnet","effort":"low"}]"#,
        None,
    ));
    assert_eq!(
        stdout,
        "\
dataset: bundled cases
c001 sonnet \"low\" hit
haiku: 0 predictions: 0 hit, 0 under, 0 over, 0 invalid, 0 unscored; accuracy 0/0 valid scored
sonnet: 1 predictions: 1 hit, 0 under, 0 over, 0 invalid, 0 unscored; accuracy 1/1 valid scored
opus: 0 predictions: 0 hit, 0 under, 0 over, 0 invalid, 0 unscored; accuracy 0/0 valid scored
fable: 0 predictions: 0 hit, 0 under, 0 over, 0 invalid, 0 unscored; accuracy 0/0 valid scored
"
    );
}

#[test]
fn grades_haiku_and_fable_predictions() {
    let cases = dataset(r#"{"id":"c001","message":"m","ok":{"haiku":"low","fable":"high-max"}}"#);
    let predictions = r#"[
        {"case":"c001","target":"haiku","effort":"low"},
        {"case":"c001","target":"fable","effort":"medium"},
        {"case":"c001","target":"sonnet","effort":"low"}
    ]"#;
    let stdout = stdout_of_success(grade_files("haiku-fable", predictions, Some(&cases)));
    let expected = "\
c001 haiku \"low\" hit
c001 fable \"medium\" under
c001 sonnet \"low\" unscored
haiku: 1 predictions: 1 hit, 0 under, 0 over, 0 invalid, 0 unscored; accuracy 1/1 valid scored
sonnet: 1 predictions: 0 hit, 0 under, 0 over, 0 invalid, 1 unscored; accuracy 0/0 valid scored
opus: 0 predictions: 0 hit, 0 under, 0 over, 0 invalid, 0 unscored; accuracy 0/0 valid scored
fable: 1 predictions: 0 hit, 1 under, 0 over, 0 invalid, 0 unscored; accuracy 0/1 valid scored
";
    assert!(stdout.ends_with(expected), "{stdout}");
}

#[test]
fn grading_leaves_the_predictions_file_unchanged() {
    let contents = r#"[{"case":"c001","target":"sonnet","effort":" High "}]"#;
    let path = temp_file("unchanged-predictions.json", contents);
    stdout_of_success(run(&["grade", path.to_str().unwrap()]));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), contents);
}

#[test]
fn rejects_bad_prediction_files() {
    for (predictions, fragment) in [
        (
            r#"[{"case":"c999","target":"sonnet","effort":"low"}]"#,
            "prediction 1 names unknown case c999",
        ),
        (
            r#"[{"case":"c001","target":"gemini","effort":"low"}]"#,
            "unknown variant `gemini`",
        ),
        (
            r#"[{"case":"c001","target":"sonnet","effort":"low","current_effort":"high"}]"#,
            "unknown field `current_effort`",
        ),
        (
            r#"[{"target":"sonnet","effort":"low"}]"#,
            "missing field `case`",
        ),
        ("[]", "no predictions"),
        ("[", "EOF"),
    ] {
        assert_grade_rejected(predictions, fragment);
    }
}

#[test]
fn grade_rejects_missing_predictions_file() {
    let out = run(&["grade", "/nonexistent/predictions.json"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("cannot read"));
}

#[test]
fn grade_rejects_invalid_dataset() {
    let out = grade_files(
        "bad-dataset",
        r#"[{"case":"c001","target":"sonnet","effort":"low"}]"#,
        Some("[]"),
    );
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("invalid dataset"));
}

// ---- run and show ----

const TEST_KEY: &str = "sk-test-secret-1234";
/// Every target answers `low`; the first one also carries probabilities and confidence.
const OPENROUTER_REPLY: &str = r#"{"model":"typesafe/jev-1.13-20260917","answers":{"effort_haiku":{"type":"choice","choice":"low","probabilities":{"low":0.83,"medium":0.17,"high":0,"xhigh":0,"max":0},"confidence":0.79},"effort_sonnet":{"type":"choice","choice":"low"},"effort_opus":{"type":"choice","choice":"low"},"effort_fable":{"type":"choice","choice":"low"}},"usage":{"input_tokens":488,"output_tokens":54,"cost":2.0496e-05},"provider":"TypeSafe"}"#;
const ZEN_429: &str = r#"{"type":"error","error":{"type":"FreeUsageLimitError","message":"Free usage limit reached."}}"#;

/// A reply where each listed target answers with its own raw JSON `choice`.
fn reply_for(choices: &[(&str, &str)]) -> String {
    let answers: Vec<String> = choices
        .iter()
        .map(|(target, choice)| {
            format!(r#""effort_{target}":{{"type":"choice","choice":{choice}}}"#)
        })
        .collect();
    format!(r#"{{"answers":{{{}}}}}"#, answers.join(","))
}

/// A reply where every target gives the same raw JSON `choice`.
fn reply(choice: &str) -> String {
    reply_for(&[
        ("haiku", choice),
        ("sonnet", choice),
        ("opus", choice),
        ("fable", choice),
    ])
}

/// A request captured by the stub, split into head (request line and headers) and JSON body.
struct Captured {
    head: String,
    body: Value,
}

impl Captured {
    fn header(&self, name: &str) -> Option<String> {
        self.head.lines().skip(1).find_map(|line| {
            let (key, value) = line.split_once(':')?;
            key.eq_ignore_ascii_case(name)
                .then(|| value.trim().to_string())
        })
    }
}

/// One scripted stub reply.
struct Reply {
    status: u16,
    body: String,
    delay: Duration,
    /// Close the connection without answering (a transport error for the client).
    drop_connection: bool,
}

impl Reply {
    fn new(status: u16, body: &str) -> Self {
        Self {
            status,
            body: body.to_string(),
            delay: Duration::ZERO,
            drop_connection: false,
        }
    }

    fn dropped() -> Self {
        Self {
            drop_connection: true,
            ..Self::new(0, "")
        }
    }

    fn delayed(mut self, delay: Duration) -> Self {
        self.delay = delay;
        self
    }
}

struct Stub {
    url: String,
    requests: Receiver<String>,
    /// The most connections the stub held open at once.
    peak: Arc<AtomicUsize>,
}

impl Stub {
    fn next(&self) -> Captured {
        let raw = self
            .requests
            .recv_timeout(Duration::from_secs(10))
            .expect("stub saw a request");
        let (head, body) = raw.split_once("\r\n\r\n").unwrap();
        Captured {
            head: head.to_string(),
            body: serde_json::from_str(body).unwrap(),
        }
    }

    /// Requests seen and not yet taken with `next`.
    fn pending(&self) -> usize {
        self.requests.try_iter().count()
    }

    fn peak(&self) -> usize {
        self.peak.load(Ordering::SeqCst)
    }
}

/// Reads one full HTTP request (head and body); `None` if the client hung up first.
fn read_request(stream: &mut TcpStream) -> Option<String> {
    let mut raw = Vec::new();
    let mut chunk = [0; 4096];
    let head_end = loop {
        let n = stream.read(&mut chunk).ok().filter(|n| *n > 0)?;
        raw.extend_from_slice(&chunk[..n]);
        if let Some(at) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
            break at + 4;
        }
    };
    let head = String::from_utf8_lossy(&raw[..head_end]).to_lowercase();
    let length: usize = head
        .lines()
        .find_map(|l| l.strip_prefix("content-length:"))
        .map_or(0, |v| v.trim().parse().unwrap());
    while raw.len() < head_end + length {
        let n = stream.read(&mut chunk).ok().filter(|n| *n > 0)?;
        raw.extend_from_slice(&chunk[..n]);
    }
    Some(String::from_utf8_lossy(&raw).into_owned())
}

/// Serves every connection from its own thread. Replies go out in request order; the last one
/// repeats. Each request is reported over a channel.
fn scripted(replies: Vec<Reply>) -> Stub {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/decisions", listener.local_addr().unwrap());
    let (sender, requests) = channel();
    let replies = Arc::new(replies);
    let served = Arc::new(AtomicUsize::new(0));
    let in_flight = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    let peak_out = Arc::clone(&peak);
    std::thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            let (sender, replies) = (sender.clone(), Arc::clone(&replies));
            let (served, in_flight, peak) = (
                Arc::clone(&served),
                Arc::clone(&in_flight),
                Arc::clone(&peak),
            );
            std::thread::spawn(move || {
                let now = in_flight.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(now, Ordering::SeqCst);
                let raw = read_request(&mut stream);
                let reply = &replies[served.fetch_add(1, Ordering::SeqCst).min(replies.len() - 1)];
                if let Some(raw) = raw {
                    let _ = sender.send(raw);
                    std::thread::sleep(reply.delay);
                }
                // Released before answering, so the next request cannot overlap this one.
                in_flight.fetch_sub(1, Ordering::SeqCst);
                if reply.drop_connection {
                    return;
                }
                let response = format!(
                    "HTTP/1.1 {} Stub\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                    reply.status,
                    reply.body.len(),
                    reply.body
                );
                let _ = stream.write_all(response.as_bytes());
            });
        }
    });
    Stub {
        url,
        requests,
        peak: peak_out,
    }
}

/// Answers every request with the same canned reply.
fn stub(status: u16, body: &str) -> Stub {
    scripted(vec![Reply::new(status, body)])
}

fn candidate_toml(name: &str, provider: &str, model: &str) -> String {
    format!(
        "[[candidate]]\nname = \"{name}\"\ndecision_model = \"jev-1.13\"\nprovider = \"{provider}\"\nmodel = \"{model}\"\n"
    )
}

fn fresh_dir(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// Files directly inside `dir`; empty when it does not exist.
fn files_in(dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir)
        .map(|entries| entries.map(|e| e.unwrap().path()).collect())
        .unwrap_or_default()
}

fn jsonl(path: &Path) -> Vec<Value> {
    std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

/// One `run` with its own config file and output directory, both named after `test`.
/// Requests go to `url` through the test hook.
struct Eval {
    out_dir: PathBuf,
    config: PathBuf,
    url: String,
}

impl Eval {
    fn new(test: &str, config_toml: &str, url: &str) -> Self {
        Self {
            out_dir: fresh_dir(&format!("{test}-out")),
            config: temp_file(&format!("{test}-config.toml"), config_toml),
            url: url.to_string(),
        }
    }

    fn run(&self, extra: &[&str], envs: &[(&str, &str)]) -> Output {
        self.command(extra, envs).output().expect("binary runs")
    }

    fn command(&self, extra: &[&str], envs: &[(&str, &str)]) -> Command {
        let mut args = vec!["run", "--config", self.config.to_str().unwrap()];
        args.extend(["--out", self.out_dir.to_str().unwrap()]);
        args.extend(extra);
        let mut envs = envs.to_vec();
        envs.push(("DECISIONBENCH_TEST_BASE_URL", &self.url));
        command_with_env(&args, &envs)
    }

    /// The only evidence file written so far.
    fn file(&self) -> PathBuf {
        let mut files = files_in(&self.out_dir);
        assert_eq!(files.len(), 1, "{files:?}");
        files.remove(0)
    }

    fn lines(&self) -> Vec<Value> {
        jsonl(&self.file())
    }

    fn manifest(&self) -> Value {
        self.lines().remove(0)
    }

    fn trials(&self) -> Vec<Value> {
        self.lines()
            .into_iter()
            .filter(|l| l["type"] == "trial")
            .collect()
    }

    fn end(&self) -> Value {
        let end = self.lines().pop().unwrap();
        assert_eq!(end["type"], "end");
        end
    }
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn openrouter_eval(test: &str, stub: &Stub) -> Eval {
    Eval::new(
        test,
        &candidate_toml("jev-openrouter", "openrouter", "typesafe/jev-1.13"),
        &stub.url,
    )
}

const C002_OPUS: [&str; 8] = [
    "--candidate",
    "jev-openrouter",
    "--case",
    "c002",
    "--target",
    "opus",
    "--repetitions",
    "1",
];

const ZEN_C002_OPUS: [&str; 8] = [
    "--candidate",
    "jev-zen",
    "--case",
    "c002",
    "--target",
    "opus",
    "--repetitions",
    "1",
];

fn args_with<'a>(base: &[&'a str], extra: &[&'a str]) -> Vec<&'a str> {
    let mut args = base.to_vec();
    args.extend(extra);
    args
}

#[test]
fn openrouter_request_is_the_documented_decisions_body() {
    let stub = stub(200, OPENROUTER_REPLY);
    let eval = openrouter_eval("or-request", &stub);
    let out = eval.run(&C002_OPUS, &[("OPENROUTER_API_KEY", TEST_KEY)]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));

    let request = stub.next();
    assert!(
        request.head.starts_with("POST /decisions "),
        "{}",
        request.head
    );
    assert_eq!(
        request.header("authorization"),
        Some(format!("Bearer {TEST_KEY}"))
    );
    let body = &request.body;
    assert_eq!(body["model"], "typesafe/jev-1.13");
    assert!(
        body["state"]["task"]
            .as_str()
            .unwrap()
            .starts_with("Choose the reasoning effort")
    );
    assert_eq!(
        body["state"]["next_message"],
        "rename the variable usr to user in auth.ts"
    );
    assert!(body["state"].get("recent_conversation").is_none());
    let raw = body.to_string();
    assert!(
        !raw.contains("current_effort")
            && !raw.contains("current_model")
            && !raw.contains("target_model"),
        "{raw}"
    );
    let questions: Vec<&String> = body["questions"].as_object().unwrap().keys().collect();
    assert_eq!(questions, ["effort_opus"]);
    let effort = &body["questions"]["effort_opus"];
    assert_eq!(effort["type"], "choice");
    assert_eq!(
        effort["instructions"],
        "Which effort should Claude Opus 5.5 use for the next message?"
    );
    let keys: Vec<&String> = effort["criteria"].as_object().unwrap().keys().collect();
    assert_eq!(keys, ["low", "medium", "high", "xhigh", "max"]);
}

#[test]
fn answered_hit_prints_the_summary_and_saves_jsonl_evidence() {
    let stub = stub(200, OPENROUTER_REPLY);
    let eval = openrouter_eval("or-hit", &stub);
    let out = eval.run(&C002_OPUS, &[("OPENROUTER_API_KEY", TEST_KEY)]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));

    let printed = stdout(&out);
    let lines: Vec<&str> = printed.lines().collect();
    assert_eq!(lines[0], "run: complete");
    assert_eq!(lines[1], "trials: 1 recorded of 1 planned");
    assert_eq!(
        lines[2],
        "candidate: jev-openrouter (openrouter typesafe/jev-1.13)"
    );
    assert!(lines[3].starts_with("  neutral ("), "{printed}");
    assert!(
        lines[3].ends_with(
            ") opus: 1 predictions: 1 hit, 0 under, 0 over, 0 invalid, 0 unscored; accuracy 1/1 valid scored"
        ),
        "{printed}"
    );
    assert_eq!(
        lines[4],
        "  outcomes: 1 answered, 0 provider_error, 0 transport_error; attempts: 1, retried trials: 0"
    );
    assert_eq!(lines[5], "  cost: unknown (no price for 1 attempt)");
    assert_eq!(lines[6], format!("evidence: {}", eval.file().display()));
    assert_eq!(lines.len(), 7);

    let progress = stderr(&out);
    assert!(
        progress.contains(
            "planned: 1 candidate × 1 instruction set × 1 case (0 unlabelled skipped) × 1 repetition = 1 trial, up to 2 requests"
        ),
        "{progress}"
    );
    assert!(
        progress.contains("[1/1] jev-openrouter neutral c002 #1: opus hit (1 attempt, "),
        "{progress}"
    );

    let lines = eval.lines();
    assert_eq!(lines.len(), 3);
    let manifest = &lines[0];
    assert_eq!(manifest["type"], "run");
    assert_eq!(manifest["schema_version"], 4);
    assert_eq!(manifest["dataset"]["name"], "bundled cases");
    assert_eq!(manifest["planned_trials"], 1);
    assert_eq!(manifest["candidates"][0]["name"], "jev-openrouter");
    assert_eq!(manifest["candidates"][0]["provider"], "openrouter");
    assert_eq!(manifest["candidates"][0]["url"], stub.url);
    assert_eq!(manifest["cases"], json!(["c002"]));
    assert_eq!(manifest["targets"], json!(["opus"]));
    assert_eq!(
        manifest["settings"],
        json!({"repetitions":1,"concurrency":1,"timeout_secs":60,"retries":1,"include_unscored":false})
    );
    assert_eq!(manifest["instructions"][0]["name"], "neutral");

    let trial = &lines[1];
    assert_eq!(trial["type"], "trial");
    assert_eq!(trial["seq"], 0);
    assert_eq!(trial["candidate"], "jev-openrouter");
    assert_eq!(trial["case_id"], "c002");
    assert_eq!(trial["targets"], json!(["opus"]));
    assert!(trial.get("target").is_none() && trial.get("label").is_none());
    assert_eq!(trial["repetition"], 1);
    assert_eq!(
        trial["labels"],
        json!({"haiku": "low", "sonnet": "low", "opus": "low", "fable": "low"})
    );
    assert_eq!(trial["instructions"]["name"], "neutral");
    assert!(trial["instructions"].get("text").is_none());
    assert!(trial["request"]["questions"]["effort_opus"].is_object());
    assert_eq!(trial["attempts"].as_array().unwrap().len(), 1);
    assert_eq!(trial["attempts"][0]["disposition"], "answered");
    assert_eq!(trial["attempts"][0]["raw_response"], OPENROUTER_REPLY);
    assert!(trial["elapsed_ms"].is_u64() && trial["started_unix_ms"].is_u64());
    assert!(
        trial.get("grade").is_none() && trial.get("prediction").is_none(),
        "grades are derived, not stored"
    );

    assert_eq!(lines[2]["type"], "end");
    assert_eq!(lines[2]["state"], "complete");
    assert!(lines[2]["finished_unix_ms"].is_u64());
}

#[test]
fn evidence_never_holds_the_key() {
    let stub = stub(200, OPENROUTER_REPLY);
    let eval = openrouter_eval("or-redact", &stub);
    let out = eval.run(&C002_OPUS, &[("OPENROUTER_API_KEY", TEST_KEY)]);
    assert_eq!(out.status.code(), Some(0));
    let file = std::fs::read_to_string(eval.file()).unwrap();
    assert!(!file.contains(TEST_KEY));
    assert!(!stdout(&out).contains(TEST_KEY) && !stderr(&out).contains(TEST_KEY));
    assert!(!file.to_lowercase().contains("authorization"));
}

/// An OpenRouter Eval whose config sits alone in its own directory, so the `credentials.toml`
/// beside it belongs to this test only.
fn eval_with_credentials(test: &str, credentials: &str, stub: &Stub) -> Eval {
    let dir = fresh_dir(test);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("config.toml"),
        candidate_toml("jev-openrouter", "openrouter", "typesafe/jev-1.13"),
    )
    .unwrap();
    std::fs::write(dir.join("credentials.toml"), credentials).unwrap();
    Eval {
        out_dir: dir.join("out"),
        config: dir.join("config.toml"),
        url: stub.url.clone(),
    }
}

#[test]
fn a_saved_key_beside_the_config_is_sent_unless_the_environment_has_one_and_stays_out_of_evidence()
{
    let stub = stub(200, OPENROUTER_REPLY);
    let eval = eval_with_credentials(
        "saved-key",
        &format!("openrouter = \"{TEST_KEY}\"\n"),
        &stub,
    );
    let out = eval.run(&C002_OPUS, &[]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(
        stub.next().header("authorization"),
        Some(format!("Bearer {TEST_KEY}"))
    );
    let file = std::fs::read_to_string(eval.file()).unwrap();
    assert!(!file.contains(TEST_KEY));
    assert!(!stdout(&out).contains(TEST_KEY) && !stderr(&out).contains(TEST_KEY));

    // A non-empty environment variable wins; an empty one does not.
    let out = eval.run(&C002_OPUS, &[("OPENROUTER_API_KEY", "env-key")]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(
        stub.next().header("authorization"),
        Some("Bearer env-key".to_string())
    );
    let out = eval.run(&C002_OPUS, &[("OPENROUTER_API_KEY", "")]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(
        stub.next().header("authorization"),
        Some(format!("Bearer {TEST_KEY}"))
    );
}

#[test]
fn a_bad_credentials_file_exits_1_naming_it_without_a_request_or_the_key() {
    let stub = stub(200, OPENROUTER_REPLY);
    let eval = eval_with_credentials(
        "bad-credentials",
        &format!("typo = \"{TEST_KEY}\"\n"),
        &stub,
    );
    let out = eval.run(&C002_OPUS, &[("OPENROUTER_API_KEY", "env-key")]);
    assert_eq!(out.status.code(), Some(1));
    let message = stderr(&out);
    let path = eval.config.with_file_name("credentials.toml");
    assert!(message.contains(&path.display().to_string()), "{message}");
    assert!(!message.contains(TEST_KEY), "{message}");
    assert!(files_in(&eval.out_dir).is_empty());
    assert!(stub.requests.try_recv().is_err(), "no request was sent");
}

#[test]
fn zen_without_a_key_sends_no_authorization_header() {
    let stub = stub(200, OPENROUTER_REPLY);
    let eval = Eval::new(
        "zen-nokey",
        &candidate_toml("jev-zen", "zen", "jev-1.13-free"),
        &stub.url,
    );
    let out = eval.run(&ZEN_C002_OPUS, &[]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let request = stub.next();
    assert_eq!(request.header("authorization"), None, "{}", request.head);
    assert_eq!(request.body["model"], "jev-1.13-free");
    assert_eq!(eval.manifest()["candidates"][0]["provider"], "zen");
}

#[test]
fn zen_with_a_key_sends_it_and_an_empty_key_counts_as_unset() {
    let stub = stub(200, OPENROUTER_REPLY);
    let eval = Eval::new(
        "zen-key",
        &candidate_toml("jev-zen", "zen", "jev-1.13"),
        &stub.url,
    );
    assert_eq!(
        eval.run(&ZEN_C002_OPUS, &[("OPENCODE_API_KEY", "")])
            .status
            .code(),
        Some(0)
    );
    assert_eq!(stub.next().header("authorization"), None);
    assert_eq!(
        eval.run(&ZEN_C002_OPUS, &[("OPENCODE_API_KEY", "zk")])
            .status
            .code(),
        Some(0)
    );
    assert_eq!(
        stub.next().header("authorization"),
        Some("Bearer zk".to_string())
    );
}

#[test]
fn one_request_per_case_asks_one_question_per_selected_target() {
    let stub = stub(200, OPENROUTER_REPLY);
    let dataset = temp_file(
        "paired-dataset.json",
        &dataset(
            r#"{"id":"p1","message":"refactor it","context":"user: earlier\nassistant: ok","ok":{"sonnet":"low","opus":"medium"}}"#,
        ),
    );
    let eval = openrouter_eval("paired", &stub);
    let out = eval.run(
        &[
            "--candidate",
            "jev-openrouter",
            "--repetitions",
            "1",
            "--dataset",
            dataset.to_str().unwrap(),
        ],
        &[("OPENROUTER_API_KEY", TEST_KEY)],
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let body = stub.next().body;
    assert_eq!(stub.pending(), 0, "one request for all four targets");
    assert_eq!(
        body["state"]["recent_conversation"],
        "user: earlier\nassistant: ok"
    );
    let questions: Vec<&String> = body["questions"].as_object().unwrap().keys().collect();
    assert_eq!(
        questions,
        [
            "effort_haiku",
            "effort_sonnet",
            "effort_opus",
            "effort_fable"
        ]
    );
    // Each question carries its own target's instructions and the same criteria keys.
    for (target, name) in [
        ("haiku", "Haiku 5.5"),
        ("sonnet", "Sonnet 5.5"),
        ("opus", "Opus 5.5"),
        ("fable", "Fable 5.1"),
    ] {
        assert_eq!(
            body["questions"][format!("effort_{target}")]["instructions"],
            format!("Which effort should Claude {name} use for the next message?")
        );
    }
    // Targets without a label are still asked and show up as unscored.
    assert!(
        stdout(&out)
            .contains(" haiku: 1 predictions: 0 hit, 0 under, 0 over, 0 invalid, 1 unscored;"),
        "{}",
        stdout(&out)
    );
    assert!(
        stdout(&out).contains(" sonnet: 1 predictions: 1 hit,"),
        "{}",
        stdout(&out)
    );
    assert!(
        stdout(&out).contains(" opus: 1 predictions: 0 hit, 1 under,"),
        "{}",
        stdout(&out)
    );
    assert!(
        stderr(&out)
            .contains("#1: haiku unscored · sonnet hit · opus under · fable unscored (1 attempt, "),
        "{}",
        stderr(&out)
    );
}

#[test]
fn each_target_is_graded_from_its_own_answer() {
    let body = reply_for(&[
        ("sonnet", r#""high""#),
        ("opus", r#""low""#),
        ("fable", r#""High""#),
    ]);
    let stub = stub(200, &body);
    let dataset = temp_file(
        "own-answer-dataset.json",
        &dataset(
            r#"{"id":"p1","message":"m","ok":{"haiku":"low","sonnet":"low","opus":"low","fable":"low"}}"#,
        ),
    );
    let eval = openrouter_eval("own-answer", &stub);
    let out = eval.run(
        &[
            "--candidate",
            "jev-openrouter",
            "--repetitions",
            "1",
            "--dataset",
            dataset.to_str().unwrap(),
        ],
        &[("OPENROUTER_API_KEY", TEST_KEY)],
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    // haiku has no answer (missing), sonnet is over, opus a hit, fable misspelled.
    assert!(
        stderr(&out).contains("#1: haiku invalid · sonnet over · opus hit · fable invalid ("),
        "{}",
        stderr(&out)
    );
}

#[test]
fn the_request_asks_only_the_selected_targets() {
    let stub = stub(200, OPENROUTER_REPLY);
    let eval = openrouter_eval("selected-targets", &stub);
    let out = eval.run(
        &[
            "--candidate",
            "jev-openrouter",
            "--case",
            "c002",
            "--repetitions",
            "1",
            "--target",
            "fable",
            "--target",
            "haiku",
            "--include-unscored",
        ],
        &[("OPENROUTER_API_KEY", TEST_KEY)],
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let body = stub.next().body;
    let questions: Vec<&String> = body["questions"].as_object().unwrap().keys().collect();
    // Always in haiku, sonnet, opus, fable order, whatever order they were given.
    assert_eq!(questions, ["effort_haiku", "effort_fable"]);
    assert_eq!(eval.trials()[0]["targets"], json!(["haiku", "fable"]));
}

#[test]
fn invalid_answers_are_graded_invalid_with_exit_0() {
    for (n, body) in [
        reply(r#""High""#),
        "{}".to_string(),
        "this is not json".to_string(),
    ]
    .into_iter()
    .enumerate()
    {
        let stub = stub(200, &body);
        let eval = openrouter_eval(&format!("invalid-{n}"), &stub);
        let out = eval.run(&C002_OPUS, &[("OPENROUTER_API_KEY", TEST_KEY)]);
        assert_eq!(out.status.code(), Some(0), "{body}: {}", stderr(&out));
        assert!(
            stdout(&out).contains(": 1 predictions: 0 hit, 0 under, 0 over, 1 invalid,"),
            "{}",
            stdout(&out)
        );
        assert!(
            stderr(&out).contains(" #1: opus invalid ("),
            "{}",
            stderr(&out)
        );
        assert_eq!(eval.trials()[0]["attempts"][0]["raw_response"], body);
    }
}

#[test]
fn provider_failure_exits_3_and_keeps_the_raw_body() {
    let stub = stub(429, ZEN_429);
    let eval = Eval::new(
        "zen-429",
        &candidate_toml("jev-zen", "zen", "jev-1.13-free"),
        &stub.url,
    );
    let out = eval.run(&args_with(&ZEN_C002_OPUS, &["--retries", "0"]), &[]);
    assert_eq!(out.status.code(), Some(3), "{}", stderr(&out));
    assert!(
        stderr(&out).contains(
            "[1/1] jev-zen neutral c002 #1: provider_error (HTTP 429): Free usage limit reached. (1 attempt, "
        ),
        "{}",
        stderr(&out)
    );
    assert!(
        stdout(&out).contains("  outcomes: 0 answered, 1 provider_error, 0 transport_error;"),
        "{}",
        stdout(&out)
    );
    let attempt = &eval.trials()[0]["attempts"][0];
    assert_eq!(attempt["disposition"], "provider_error");
    assert_eq!(attempt["http_status"], 429);
    assert_eq!(attempt["raw_response"], ZEN_429);
    assert_eq!(eval.end()["state"], "complete");
}

#[test]
fn transport_failure_exits_3_and_saves_evidence() {
    // A server that accepts and hangs up without answering: a transport error that does not depend
    // on how the host treats connections to a closed port (some sandboxes drop them silently).
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            drop(stream);
        }
    });
    let eval = Eval::new(
        "transport",
        &candidate_toml("jev-openrouter", "openrouter", "m"),
        &url,
    );
    let out = eval.run(
        &args_with(&C002_OPUS, &["--retries", "0"]),
        &[("OPENROUTER_API_KEY", TEST_KEY)],
    );
    assert_eq!(out.status.code(), Some(3), "{}", stderr(&out));
    assert!(
        stderr(&out).contains(" #1: transport_error: "),
        "{}",
        stderr(&out)
    );
    assert!(
        stdout(&out).contains("  outcomes: 0 answered, 0 provider_error, 1 transport_error;"),
        "{}",
        stdout(&out)
    );
    let attempt = &eval.trials()[0]["attempts"][0];
    assert_eq!(attempt["disposition"], "transport_error");
    assert!(attempt["error"].as_str().is_some_and(|e| !e.is_empty()));
    assert_eq!(attempt["http_status"], Value::Null);
}

#[test]
fn missing_openrouter_key_exits_1_without_a_request_or_file() {
    let stub = stub(200, OPENROUTER_REPLY);
    let eval = openrouter_eval("no-key", &stub);
    let out = eval.run(&C002_OPUS, &[]);
    assert_eq!(out.status.code(), Some(1));
    let credentials = eval.config.with_file_name("credentials.toml");
    assert!(
        stderr(&out).contains(&format!(
            "OPENROUTER_API_KEY is not set (and no openrouter key in {})",
            credentials.display()
        )),
        "{}",
        stderr(&out)
    );
    assert!(out.stdout.is_empty());
    assert!(files_in(&eval.out_dir).is_empty());
    assert!(stub.requests.try_recv().is_err(), "no request was sent");
}

#[test]
fn a_missing_key_for_any_selected_candidate_sends_nothing() {
    let stub = stub(200, OPENROUTER_REPLY);
    let config = format!(
        "{}{}",
        candidate_toml("free", "zen", "m"),
        candidate_toml("paid", "openrouter", "m")
    );
    let eval = Eval::new("no-key-second", &config, &stub.url);
    let out = eval.run(
        &[
            "--candidate",
            "free",
            "--candidate",
            "paid",
            "--case",
            "c002",
            "--target",
            "opus",
            "--repetitions",
            "1",
        ],
        &[],
    );
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert!(stderr(&out).contains("OPENROUTER_API_KEY"));
    assert!(files_in(&eval.out_dir).is_empty());
    assert!(stub.requests.try_recv().is_err(), "no request was sent");
}

#[test]
fn input_errors_exit_1_before_any_request() {
    let stub = stub(200, OPENROUTER_REPLY);
    let eval = openrouter_eval("inputs", &stub);
    let unknown_field = Eval::new(
        "inputs-field",
        &format!("{}colour = \"red\"\n", candidate_toml("c", "zen", "m")),
        &stub.url,
    );
    let unknown_provider = Eval::new(
        "inputs-provider",
        &candidate_toml("c", "bing", "m"),
        &stub.url,
    );
    let two_zens = Eval::new(
        "inputs-two",
        &format!(
            "{}{}",
            candidate_toml("a", "zen", "m"),
            candidate_toml("b", "zen", "m")
        ),
        &stub.url,
    );
    let prompts = fresh_dir("inputs-prompts");
    std::fs::create_dir_all(prompts.join("x")).unwrap();
    std::fs::create_dir_all(prompts.join("y")).unwrap();
    std::fs::write(prompts.join("x/same.md"), custom()).unwrap();
    std::fs::write(prompts.join("y/same.md"), custom().replace("low desc", "o")).unwrap();
    let (x, y) = (prompts.join("x/same.md"), prompts.join("y/same.md"));
    let case_opus = ["--case", "c002", "--target", "opus"];
    let c = |name: &'static str| vec!["--candidate", name];
    for (eval, args, fragment) in [
        (
            &eval,
            [c("nope"), case_opus.to_vec()].concat(),
            "unknown candidate `nope`",
        ),
        (
            &eval,
            vec!["--candidate", "jev-openrouter", "--case", "c999"],
            "unknown case c999",
        ),
        (
            &eval,
            vec![
                "--candidate",
                "jev-openrouter",
                "--candidate",
                "jev-openrouter",
            ],
            "duplicate candidate `jev-openrouter`",
        ),
        (
            &eval,
            vec![
                "--candidate",
                "jev-openrouter",
                "--case",
                "c002",
                "--case",
                "c002",
            ],
            "duplicate case `c002`",
        ),
        (
            &eval,
            vec![
                "--candidate",
                "jev-openrouter",
                "--target",
                "opus",
                "--target",
                "opus",
            ],
            "duplicate target `opus`",
        ),
        (
            &eval,
            vec![
                "--candidate",
                "jev-openrouter",
                "--instructions",
                x.to_str().unwrap(),
                "--instructions",
                y.to_str().unwrap(),
            ],
            "duplicate instructions name `same`",
        ),
        (
            &two_zens,
            vec!["--candidate", "a", "--candidate", "nope"],
            "unknown candidate `nope`",
        ),
        (
            &unknown_field,
            [c("c"), case_opus.to_vec()].concat(),
            "unknown field `colour`",
        ),
        (
            &unknown_provider,
            [c("c"), case_opus.to_vec()].concat(),
            "unknown variant `bing`",
        ),
    ] {
        let out = eval.run(&args, &[("OPENROUTER_API_KEY", TEST_KEY)]);
        assert_eq!(out.status.code(), Some(1), "{fragment}: {}", stderr(&out));
        assert!(
            stderr(&out).contains(fragment),
            "{fragment} not in {}",
            stderr(&out)
        );
        assert!(files_in(&eval.out_dir).is_empty(), "{fragment}");
    }
    assert!(stub.requests.try_recv().is_err(), "no request was sent");
    let missing = run(&[
        "run",
        "--config",
        "/nonexistent/config.toml",
        "--candidate",
        "c",
    ]);
    assert_eq!(missing.status.code(), Some(1));
    assert!(stderr(&missing).contains("cannot read /nonexistent/config.toml"));
}

#[test]
fn usage_errors_exit_2() {
    for args in [
        vec!["run", "--candidate", "c", "--target", "gemini"],
        vec!["run"],
        vec!["run", "--candidate", "c", "--repetitions", "0"],
        vec!["run", "--candidate", "c", "--concurrency", "0"],
        vec!["run", "--candidate", "c", "--timeout", "0"],
        vec!["run", "--candidate", "c", "--retries", "-1"],
        vec!["evaluate", "--candidate", "c"],
    ] {
        assert_eq!(run(&args).status.code(), Some(2), "{args:?}");
    }
}

#[test]
fn every_target_name_is_accepted_by_run() {
    // A run that fails on the missing config proves the argument parsed (exit 1, not 2).
    for target in ["haiku", "sonnet", "opus", "fable"] {
        let out = run(&[
            "run",
            "--config",
            "/nonexistent/config.toml",
            "--candidate",
            "c",
            "--target",
            target,
        ]);
        assert_eq!(out.status.code(), Some(1), "{target}: {}", stderr(&out));
    }
}

#[test]
fn show_prints_the_full_report_and_leaves_the_file_unchanged() {
    let stub = stub(200, OPENROUTER_REPLY);
    let eval = openrouter_eval("show", &stub);
    let out = eval.run(&C002_OPUS, &[("OPENROUTER_API_KEY", TEST_KEY)]);
    assert_eq!(out.status.code(), Some(0));
    let file = eval.file();
    let before = std::fs::read(&file).unwrap();

    let shown = run(&["show", file.to_str().unwrap()]);
    assert_eq!(shown.status.code(), Some(0), "{}", stderr(&shown));
    assert!(
        stdout(&shown).starts_with("== Run ==\n"),
        "{}",
        stdout(&shown)
    );
    assert_has_line(&stdout(&shown), "state: complete");
    assert_eq!(std::fs::read(&file).unwrap(), before);
}

#[test]
fn show_regrades_from_the_stored_label_and_raw_effort() {
    let stub = stub(200, &reply(r#""max""#));
    let eval = openrouter_eval("show-regrade", &stub);
    eval.run(&C002_OPUS, &[("OPENROUTER_API_KEY", TEST_KEY)]);
    let shown = run(&["show", eval.file().to_str().unwrap()]);
    assert!(
        stdout(&shown).contains("opus: hit 0, under 0, over 1, invalid 0, unscored 0; n=1"),
        "{}",
        stdout(&shown)
    );
}

#[test]
fn show_rejects_unknown_schema_versions_and_bad_files() {
    let stub = stub(200, OPENROUTER_REPLY);
    let eval = openrouter_eval("show-schema", &stub);
    eval.run(&C002_OPUS, &[("OPENROUTER_API_KEY", TEST_KEY)]);
    let mut lines = eval.lines();
    lines[0]["schema_version"] = json!(99);
    let future = temp_file("future-evidence.jsonl", &join_lines(&lines));
    let out = run(&["show", future.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("schema_version 99"),
        "{}",
        stderr(&out)
    );
    assert!(out.stdout.is_empty());

    // The old single-evaluation file: one pretty-printed JSON object.
    let v1 = temp_file(
        "v1-evidence.json",
        &serde_json::to_string_pretty(&json!({"schema_version": 1, "id": "1-2"})).unwrap(),
    );
    let out = run(&["show", v1.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("schema_version 1"),
        "{}",
        stderr(&out)
    );

    // A file that does not start with the run manifest.
    let mut lines = eval.lines();
    lines.remove(0);
    let headless = temp_file("headless-evidence.jsonl", &join_lines(&lines));
    assert_eq!(
        run(&["show", headless.to_str().unwrap()]).status.code(),
        Some(1)
    );

    // An unparseable line in the middle is an error, not a warning.
    let mut text: Vec<String> = std::fs::read_to_string(eval.file())
        .unwrap()
        .lines()
        .map(String::from)
        .collect();
    text.insert(1, "{broken".to_string());
    let middle = temp_file("middle-evidence.jsonl", &(text.join("\n") + "\n"));
    let out = run(&["show", middle.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("line 2"), "{}", stderr(&out));

    let junk = temp_file("junk-evidence.jsonl", "{");
    assert_eq!(
        run(&["show", junk.to_str().unwrap()]).status.code(),
        Some(1)
    );
    assert_eq!(
        run(&["show", "/nonexistent/e.jsonl"]).status.code(),
        Some(1)
    );
}

fn join_lines(lines: &[Value]) -> String {
    lines.iter().map(|l| format!("{l}\n")).collect()
}

fn hex_sha256(text: &str) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(text.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn custom_text(targets: &[&str]) -> String {
    let mut text = "Pick the effort for this exact test.\n".to_string();
    for target in targets {
        text.push_str(&format!(
            "\n## {target}\nQuestion for {target}?\n\n- low: low desc\n- medium: medium desc\n- high: high desc\n- xhigh: xhigh desc\n- max: max desc\n"
        ));
    }
    text
}

fn custom() -> String {
    custom_text(&["haiku", "sonnet", "opus", "fable"])
}

#[test]
fn custom_instructions_are_sent_and_recorded_by_name_and_hash() {
    let stub = stub(200, OPENROUTER_REPLY);
    let eval = openrouter_eval("custom", &stub);
    let dir = fresh_dir("custom-prompts");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("custom.md");
    std::fs::write(&path, custom()).unwrap();
    let args = args_with(&C002_OPUS, &["--instructions", path.to_str().unwrap()]);
    let out = eval.run(&args, &[("OPENROUTER_API_KEY", TEST_KEY)]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));

    let body = stub.next().body;
    assert_eq!(
        body["state"]["task"],
        "Pick the effort for this exact test."
    );
    let effort = &body["questions"]["effort_opus"];
    assert_eq!(effort["instructions"], "Question for opus?");
    assert_eq!(
        effort["criteria"],
        json!({"low":"low desc","medium":"medium desc","high":"high desc","xhigh":"xhigh desc","max":"max desc"})
    );
    let hash = hex_sha256(&custom());
    let manifest = eval.manifest();
    assert_eq!(manifest["instructions"][0]["name"], "custom");
    assert_eq!(manifest["instructions"][0]["sha256"], hash);
    assert_eq!(manifest["instructions"][0]["text"], custom());
    let trial = &eval.trials()[0];
    assert_eq!(
        trial["instructions"],
        json!({"name": "custom", "sha256": hash})
    );
    assert!(stdout(&out).contains(&format!("  custom ({}) opus:", &hash[..12])));
}

#[test]
fn every_instructions_flag_adds_an_instruction_set_to_the_plan() {
    let stub = stub(200, OPENROUTER_REPLY);
    let eval = openrouter_eval("two-sets", &stub);
    let dir = fresh_dir("two-sets-prompts");
    std::fs::create_dir_all(&dir).unwrap();
    let (a, b) = (dir.join("a.md"), dir.join("b.md"));
    std::fs::write(&a, custom()).unwrap();
    std::fs::write(&b, custom().replace("low desc", "other desc")).unwrap();
    let args = args_with(
        &C002_OPUS,
        &[
            "--instructions",
            a.to_str().unwrap(),
            "--instructions",
            b.to_str().unwrap(),
        ],
    );
    let out = eval.run(&args, &[("OPENROUTER_API_KEY", TEST_KEY)]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert!(
        stderr(&out).contains(
            "planned: 1 candidate × 2 instruction sets × 1 case (0 unlabelled skipped) × 1 repetition = 2 trials, up to 4 requests"
        ),
        "{}",
        stderr(&out)
    );
    let manifest = eval.manifest();
    assert_eq!(manifest["planned_trials"], 2);
    let names: Vec<_> = manifest["instructions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["name"].clone())
        .collect();
    assert_eq!(names, [json!("a"), json!("b")]);
}

#[test]
fn default_instructions_are_recorded_as_neutral() {
    let stub = stub(200, OPENROUTER_REPLY);
    let eval = openrouter_eval("neutral", &stub);
    eval.run(&C002_OPUS, &[("OPENROUTER_API_KEY", TEST_KEY)]);
    let manifest = eval.manifest();
    let bundled = std::fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("prompts/neutral.md"),
    )
    .unwrap();
    assert_eq!(manifest["instructions"].as_array().unwrap().len(), 1);
    assert_eq!(manifest["instructions"][0]["name"], "neutral");
    assert_eq!(manifest["instructions"][0]["text"], bundled);
    assert_eq!(manifest["instructions"][0]["sha256"], hex_sha256(&bundled));
}

#[test]
fn malformed_instructions_exit_1_naming_the_file() {
    let stub = stub(200, OPENROUTER_REPLY);
    let eval = openrouter_eval("bad-instructions", &stub);
    let path = temp_file(
        "bad-instructions.md",
        "Intro.\n\n## opus\nQ?\n\n- low: only low\n",
    );
    let args = args_with(&C002_OPUS, &["--instructions", path.to_str().unwrap()]);
    let out = eval.run(&args, &[("OPENROUTER_API_KEY", TEST_KEY)]);
    assert_eq!(out.status.code(), Some(1));
    let err = stderr(&out);
    assert!(
        err.contains("bad-instructions.md") && err.contains("missing `- medium:`"),
        "{err}"
    );
    assert!(files_in(&eval.out_dir).is_empty());
    assert!(stub.requests.try_recv().is_err());
}

#[test]
fn instructions_missing_a_selected_target_exit_1_before_any_request() {
    let stub = stub(200, OPENROUTER_REPLY);
    let eval = openrouter_eval("missing-target", &stub);
    let path = temp_file("only-opus.md", &custom_text(&["opus"]));
    let out = eval.run(
        &args_with(
            &C002_OPUS,
            &[
                "--instructions",
                path.to_str().unwrap(),
                "--target",
                "sonnet",
            ],
        ),
        &[("OPENROUTER_API_KEY", TEST_KEY)],
    );
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    let err = stderr(&out);
    assert!(
        err.contains("instructions `only-opus`") && err.contains("no section for target sonnet"),
        "{err}"
    );
    assert!(files_in(&eval.out_dir).is_empty());
    assert!(stub.requests.try_recv().is_err(), "no request was sent");

    // Without the missing target selected, the same file runs.
    let out = eval.run(
        &args_with(&C002_OPUS, &["--instructions", path.to_str().unwrap()]),
        &[("OPENROUTER_API_KEY", TEST_KEY)],
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
}

#[test]
fn unwritable_out_exits_1_before_any_request() {
    let stub = stub(200, OPENROUTER_REPLY);
    let mut eval = openrouter_eval("bad-out", &stub);
    let blocker = temp_file("bad-out-blocker", "a regular file");
    eval.out_dir = blocker.join("runs");
    let out = eval.run(&C002_OPUS, &[("OPENROUTER_API_KEY", TEST_KEY)]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert!(stderr(&out).contains("cannot create"), "{}", stderr(&out));
    assert!(
        stub.requests
            .recv_timeout(Duration::from_millis(500))
            .is_err(),
        "no request was sent"
    );
}

#[test]
fn evidence_is_private_to_the_user() {
    use std::os::unix::fs::PermissionsExt;
    let stub = stub(200, OPENROUTER_REPLY);
    let eval = openrouter_eval("perms", &stub);
    let out = eval.run(&C002_OPUS, &[("OPENROUTER_API_KEY", TEST_KEY)]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&eval.file()), 0o600);
    assert_eq!(mode(&eval.out_dir), 0o700);
}

#[test]
fn a_truncated_body_keeps_the_http_status() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/decisions", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            // Drain the request so closing does not reset the connection before the reply lands.
            let _ = read_request(&mut stream);
            let _ = stream.write_all(
                b"HTTP/1.1 200 OK\r\ncontent-length: 1000\r\nconnection: close\r\n\r\nshort",
            );
        }
    });
    let eval = Eval::new(
        "truncated",
        &candidate_toml("jev-openrouter", "openrouter", "m"),
        &url,
    );
    let out = eval.run(
        &args_with(&C002_OPUS, &["--retries", "0"]),
        &[("OPENROUTER_API_KEY", TEST_KEY)],
    );
    assert_eq!(out.status.code(), Some(3), "{}", stderr(&out));
    let attempt = &eval.trials()[0]["attempts"][0];
    assert_eq!(attempt["disposition"], "transport_error");
    assert_eq!(attempt["http_status"], 200);
    assert!(attempt["error"].as_str().is_some_and(|e| !e.is_empty()));
}

#[test]
fn a_non_utf8_error_body_is_kept_as_a_provider_error() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/decisions", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            // Drain the whole request first: closing with unread request bytes makes the kernel
            // reset the connection, and the client then loses the reply body.
            let _ = read_request(&mut stream);
            let body = b"Bad gateway: caf\xe9 down";
            let head = format!(
                "HTTP/1.1 502 Bad Gateway\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(head.as_bytes());
            let _ = stream.write_all(body);
        }
    });
    let eval = Eval::new(
        "non-utf8",
        &candidate_toml("jev-openrouter", "openrouter", "m"),
        &url,
    );
    let out = eval.run(
        &args_with(&C002_OPUS, &["--retries", "0"]),
        &[("OPENROUTER_API_KEY", TEST_KEY)],
    );
    assert_eq!(out.status.code(), Some(3), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("provider_error (HTTP 502)"),
        "{}",
        stderr(&out)
    );
    let attempt = &eval.trials()[0]["attempts"][0];
    assert_eq!(attempt["raw_response"], "Bad gateway: caf\u{FFFD} down");
}

#[test]
fn a_key_echoed_by_the_provider_is_redacted() {
    let body = format!(r#"{{"error":{{"message":"Invalid key {TEST_KEY}"}}}}"#);
    let stub = stub(401, &body);
    let eval = openrouter_eval("echo-key", &stub);
    let out = eval.run(&C002_OPUS, &[("OPENROUTER_API_KEY", TEST_KEY)]);
    assert_eq!(out.status.code(), Some(3), "{}", stderr(&out));
    let file = std::fs::read_to_string(eval.file()).unwrap();
    assert!(!file.contains(TEST_KEY), "{file}");
    assert!(file.contains("Invalid key [redacted]"), "{file}");
    assert!(!stdout(&out).contains(TEST_KEY) && !stderr(&out).contains(TEST_KEY));
    assert!(
        stderr(&out).contains("Invalid key [redacted]"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn terminal_output_strips_control_characters_but_evidence_keeps_them() {
    for (n, body) in [
        r#"{"error":{"message":"bad \u001b[31mred"}}"#,
        "oops \u{1b}[31mred",
    ]
    .into_iter()
    .enumerate()
    {
        let stub = stub(500, body);
        let eval = openrouter_eval(&format!("ctrl-{n}"), &stub);
        let out = eval.run(
            &args_with(&C002_OPUS, &["--retries", "0"]),
            &[("OPENROUTER_API_KEY", TEST_KEY)],
        );
        assert_eq!(out.status.code(), Some(3), "{}", stderr(&out));
        assert!(!stdout(&out).contains('\u{1b}'), "{:?}", stdout(&out));
        assert!(!stderr(&out).contains('\u{1b}'), "{:?}", stderr(&out));
        assert!(stderr(&out).contains("[31mred"), "{:?}", stderr(&out));
        assert_eq!(eval.trials()[0]["attempts"][0]["raw_response"], body);
    }
}

#[test]
fn zen_auth_error_exits_3_with_the_provider_message() {
    let stub = stub(
        401,
        r#"{"type":"error","error":{"type":"AuthError","message":"Missing API key."}}"#,
    );
    let eval = Eval::new(
        "zen-401",
        &candidate_toml("jev-zen", "zen", "jev-1.13"),
        &stub.url,
    );
    let out = eval.run(&ZEN_C002_OPUS, &[]);
    assert_eq!(out.status.code(), Some(3), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("provider_error (HTTP 401): Missing API key. ("),
        "{}",
        stderr(&out)
    );
    assert_eq!(stub.pending(), 1, "401 is not retried");
}

#[test]
fn non_json_error_body_is_shown_cut_to_200_characters() {
    let body = "é".repeat(300);
    let stub = stub(502, &body);
    let eval = openrouter_eval("bad-gateway", &stub);
    let out = eval.run(
        &args_with(&C002_OPUS, &["--retries", "0"]),
        &[("OPENROUTER_API_KEY", TEST_KEY)],
    );
    assert_eq!(out.status.code(), Some(3), "{}", stderr(&out));
    let expected = format!("(HTTP 502): {}... (", "é".repeat(200));
    assert!(stderr(&out).contains(&expected), "{}", stderr(&out));
    assert_eq!(eval.trials()[0]["attempts"][0]["raw_response"], body);
}

#[test]
fn same_named_instruction_files_are_told_apart_by_hash() {
    let mut recorded = Vec::new();
    for (n, text) in [custom(), custom().replace("low desc", "other desc")]
        .into_iter()
        .enumerate()
    {
        let stub = stub(200, OPENROUTER_REPLY);
        let eval = openrouter_eval(&format!("samename-{n}"), &stub);
        let dir = fresh_dir(&format!("samename-prompts-{n}"));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("custom.md");
        std::fs::write(&path, &text).unwrap();
        let args = args_with(&C002_OPUS, &["--instructions", path.to_str().unwrap()]);
        let out = eval.run(&args, &[("OPENROUTER_API_KEY", TEST_KEY)]);
        assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
        let manifest = eval.manifest();
        assert_eq!(manifest["instructions"][0]["name"], "custom");
        recorded.push(manifest["instructions"][0]["sha256"].clone());
    }
    assert_ne!(recorded[0], recorded[1]);
}

// ---- selection, repetitions and unlabelled pairs ----

/// `k1` and `k2` are labelled for sonnet and opus; `u1` only for sonnet.
fn selection_dataset(name: &str) -> PathBuf {
    temp_file(
        name,
        &dataset(
            r#"{"id":"k1","message":"first","ok":{"sonnet":"low","opus":"low"}},
               {"id":"k2","message":"second","ok":{"sonnet":"low","opus":"low"}},
               {"id":"u1","message":"third","ok":{"sonnet":"low"}}"#,
        ),
    )
}

#[test]
fn a_full_matrix_runs_every_trial_once_repetition_first() {
    let stub = stub(200, OPENROUTER_REPLY);
    let config = format!(
        "{}{}",
        candidate_toml("a", "openrouter", "m-a"),
        candidate_toml("b", "openrouter", "m-b")
    );
    let eval = Eval::new("matrix", &config, &stub.url);
    let dir = fresh_dir("matrix-prompts");
    std::fs::create_dir_all(&dir).unwrap();
    let (one, two) = (dir.join("one.md"), dir.join("two.md"));
    std::fs::write(&one, custom()).unwrap();
    std::fs::write(&two, custom().replace("low desc", "other desc")).unwrap();
    let dataset = selection_dataset("matrix-dataset.json");
    let out = eval.run(
        &[
            "--candidate",
            "a",
            "--candidate",
            "b",
            "--instructions",
            one.to_str().unwrap(),
            "--instructions",
            two.to_str().unwrap(),
            "--case",
            "k1",
            "--case",
            "k2",
            "--repetitions",
            "3",
            "--concurrency",
            "4",
            "--dataset",
            dataset.to_str().unwrap(),
        ],
        &[("OPENROUTER_API_KEY", TEST_KEY)],
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert!(
        stderr(&out).contains(
            "planned: 2 candidates × 2 instruction sets × 2 cases (0 unlabelled skipped) × 3 repetitions = 24 trials, up to 48 requests"
        ),
        "{}",
        stderr(&out)
    );
    assert_eq!(stub.pending(), 24);
    assert_eq!(eval.manifest()["planned_trials"], 24);
    assert!(stdout(&out).contains("trials: 24 recorded of 24 planned"));

    let mut trials = eval.trials();
    assert_eq!(trials.len(), 24);
    trials.sort_by_key(|t| t["seq"].as_u64().unwrap());
    let key = |t: &Value| {
        format!(
            "{} {} {} {}",
            t["candidate"], t["instructions"]["name"], t["case_id"], t["repetition"]
        )
    };
    let distinct: std::collections::HashSet<String> = trials.iter().map(key).collect();
    assert_eq!(distinct.len(), 24);
    for (i, t) in trials.iter().enumerate() {
        assert_eq!(t["seq"], i);
        // Repetition 1 of every combination, then repetition 2, then 3.
        assert_eq!(t["repetition"], i / 8 + 1, "{}", key(t));
    }
    // Each wire model reached only its own candidate's trials.
    for t in &trials {
        let model = if t["candidate"] == "a" { "m-a" } else { "m-b" };
        assert_eq!(t["request"]["model"], model);
    }
}

#[test]
fn repetitions_default_to_three() {
    let stub = stub(200, OPENROUTER_REPLY);
    let eval = openrouter_eval("default-reps", &stub);
    let out = eval.run(
        &[
            "--candidate",
            "jev-openrouter",
            "--case",
            "c002",
            "--target",
            "opus",
        ],
        &[("OPENROUTER_API_KEY", TEST_KEY)],
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(eval.trials().len(), 3);
    assert_eq!(stub.pending(), 3);
    assert_eq!(eval.manifest()["settings"]["repetitions"], 3);
}

#[test]
fn cases_with_no_selected_target_labelled_are_skipped_by_default_and_counted() {
    let stub = stub(200, OPENROUTER_REPLY);
    let dataset = selection_dataset("skip-dataset.json");
    let eval = openrouter_eval("skip", &stub);
    let out = eval.run(
        &[
            "--candidate",
            "jev-openrouter",
            "--case",
            "k1",
            "--case",
            "u1",
            "--target",
            "opus",
            "--repetitions",
            "1",
            "--dataset",
            dataset.to_str().unwrap(),
        ],
        &[("OPENROUTER_API_KEY", TEST_KEY)],
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert!(
        stderr(&out).contains(
            "planned: 1 candidate × 1 instruction set × 1 case (1 unlabelled skipped) × 1 repetition = 1 trial, up to 2 requests"
        ),
        "{}",
        stderr(&out)
    );
    let trials = eval.trials();
    assert_eq!(trials.len(), 1);
    assert_eq!(trials[0]["case_id"], "k1");
    assert_eq!(stub.pending(), 1);
    assert_eq!(eval.manifest()["cases"], json!(["k1", "u1"]));
}

#[test]
fn a_case_is_kept_when_any_selected_target_is_labelled() {
    let stub = stub(200, OPENROUTER_REPLY);
    let dataset = selection_dataset("any-labelled-dataset.json");
    let eval = openrouter_eval("any-labelled", &stub);
    let out = eval.run(
        &[
            "--candidate",
            "jev-openrouter",
            "--case",
            "k1",
            "--case",
            "u1",
            "--repetitions",
            "1",
            "--dataset",
            dataset.to_str().unwrap(),
        ],
        &[("OPENROUTER_API_KEY", TEST_KEY)],
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("2 cases (0 unlabelled skipped) × 1 repetition = 2 trials"),
        "{}",
        stderr(&out)
    );
    assert_eq!(eval.trials().len(), 2);
    assert_eq!(stub.pending(), 2);
}

#[test]
fn include_unscored_adds_the_cases_no_selected_target_labels() {
    let stub = stub(200, OPENROUTER_REPLY);
    let dataset = selection_dataset("unscored-dataset.json");
    let eval = openrouter_eval("unscored", &stub);
    let out = eval.run(
        &[
            "--candidate",
            "jev-openrouter",
            "--case",
            "k1",
            "--case",
            "u1",
            "--target",
            "opus",
            "--repetitions",
            "1",
            "--include-unscored",
            "--dataset",
            dataset.to_str().unwrap(),
        ],
        &[("OPENROUTER_API_KEY", TEST_KEY)],
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("2 cases (0 unlabelled skipped) × 1 repetition = 2 trials"),
        "{}",
        stderr(&out)
    );
    let trials = eval.trials();
    assert_eq!(trials.len(), 2);
    let unlabelled: Vec<&Value> = trials
        .iter()
        .filter(|t| t["labels"].get("opus").is_none())
        .collect();
    assert_eq!(unlabelled.len(), 1);
    assert_eq!(unlabelled[0]["case_id"], "u1");
    assert_eq!(unlabelled[0]["labels"], json!({"sonnet": "low"}));
    assert_eq!(eval.manifest()["settings"]["include_unscored"], true);
    assert!(
        stdout(&out)
            .contains("opus: 2 predictions: 1 hit, 0 under, 0 over, 0 invalid, 1 unscored;"),
        "{}",
        stdout(&out)
    );
}

#[test]
fn selecting_only_unlabelled_cases_is_an_error_mentioning_include_unscored() {
    let stub = stub(200, OPENROUTER_REPLY);
    let dataset = selection_dataset("only-unlabelled-dataset.json");
    let eval = openrouter_eval("only-unlabelled", &stub);
    let out = eval.run(
        &[
            "--candidate",
            "jev-openrouter",
            "--case",
            "u1",
            "--target",
            "opus",
            "--dataset",
            dataset.to_str().unwrap(),
        ],
        &[("OPENROUTER_API_KEY", TEST_KEY)],
    );
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("--include-unscored"),
        "{}",
        stderr(&out)
    );
    assert!(files_in(&eval.out_dir).is_empty());
    assert!(stub.requests.try_recv().is_err(), "no request was sent");
}

// ---- concurrency and timeout ----

const SIX_TRIALS: [&str; 10] = [
    "--candidate",
    "jev-openrouter",
    "--case",
    "c001",
    "--case",
    "c003",
    "--target",
    "sonnet",
    "--repetitions",
    "3",
];

#[test]
fn concurrency_bounds_simultaneous_requests() {
    // The delay must dwarf scheduling jitter so two workers reliably overlap.
    for (n, concurrency, expected_peak, delay_ms) in [(0, "2", 2, 1000), (1, "1", 1, 100)] {
        let stub = scripted(vec![
            Reply::new(200, OPENROUTER_REPLY).delayed(Duration::from_millis(delay_ms)),
        ]);
        let eval = openrouter_eval(&format!("concurrency-{n}"), &stub);
        let out = eval.run(
            &args_with(&SIX_TRIALS, &["--concurrency", concurrency]),
            &[("OPENROUTER_API_KEY", TEST_KEY)],
        );
        assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
        assert_eq!(eval.trials().len(), 6);
        assert_eq!(stub.peak(), expected_peak, "--concurrency {concurrency}");
        assert_eq!(eval.manifest()["settings"]["concurrency"], expected_peak);
    }
}

#[test]
fn timeout_gives_a_transport_error_without_waiting_for_the_reply() {
    let stub = scripted(vec![
        Reply::new(200, OPENROUTER_REPLY).delayed(Duration::from_secs(3)),
    ]);
    let eval = openrouter_eval("timeout", &stub);
    let clock = std::time::Instant::now();
    let out = eval.run(
        &args_with(&C002_OPUS, &["--timeout", "1", "--retries", "0"]),
        &[("OPENROUTER_API_KEY", TEST_KEY)],
    );
    assert!(
        clock.elapsed() < Duration::from_millis(2900),
        "{:?}",
        clock.elapsed()
    );
    assert_eq!(out.status.code(), Some(3), "{}", stderr(&out));
    let attempt = &eval.trials()[0]["attempts"][0];
    assert_eq!(attempt["disposition"], "transport_error");
    assert!((900..2500).contains(&attempt["latency_ms"].as_u64().unwrap()));
    assert_eq!(eval.manifest()["settings"]["timeout_secs"], 1);
}

// ---- cancellation ----

/// A `run` started in the background, with its stderr collected by a thread.
struct Live {
    child: Child,
    stderr: Arc<Mutex<String>>,
    reader: std::thread::JoinHandle<()>,
}

impl Live {
    fn start(eval: &Eval, extra: &[&str]) -> Self {
        let mut command = eval.command(extra, &[("OPENROUTER_API_KEY", TEST_KEY)]);
        let mut child = command
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .expect("binary starts");
        let mut pipe = child.stderr.take().unwrap();
        let stderr = Arc::new(Mutex::new(String::new()));
        let sink = Arc::clone(&stderr);
        let reader = std::thread::spawn(move || {
            let mut chunk = [0; 1024];
            while let Ok(n) = pipe.read(&mut chunk) {
                if n == 0 {
                    break;
                }
                sink.lock()
                    .unwrap()
                    .push_str(&String::from_utf8_lossy(&chunk[..n]));
            }
        });
        Self {
            child,
            stderr,
            reader,
        }
    }

    fn signal(&self, name: &str) {
        let pid = self.child.id().to_string();
        let sent = Command::new("kill")
            .args([&format!("-{name}"), &pid])
            .status()
            .unwrap();
        assert!(sent.success());
    }

    fn sigint(&self) {
        self.signal("INT");
    }

    fn wait_for_stderr(&self, needle: &str) {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while !self.stderr.lock().unwrap().contains(needle) {
            assert!(
                std::time::Instant::now() < deadline,
                "no `{needle}` in stderr: {}",
                self.stderr.lock().unwrap()
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// The exit code and everything written to stderr; kills the child if it outlives `limit`.
    fn finish(mut self, limit: Duration) -> (Option<i32>, String) {
        let deadline = std::time::Instant::now() + limit;
        let status = loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                break status;
            }
            if std::time::Instant::now() >= deadline {
                self.child.kill().unwrap();
                self.child.wait().unwrap();
                panic!(
                    "run did not exit within {limit:?}: {}",
                    self.stderr.lock().unwrap()
                );
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        self.reader.join().unwrap();
        let stderr = self.stderr.lock().unwrap().clone();
        (status.code(), stderr)
    }
}

#[test]
fn one_sigint_finishes_in_flight_trials_then_ends_partial_with_exit_130() {
    one_signal_ends_partial_with_exit_130("INT", "sigint-once");
}

#[test]
fn one_sigterm_finishes_in_flight_trials_then_ends_partial_with_exit_130() {
    one_signal_ends_partial_with_exit_130("TERM", "sigterm-once");
}

fn one_signal_ends_partial_with_exit_130(signal: &str, name: &str) {
    let stub = scripted(vec![
        Reply::new(200, OPENROUTER_REPLY).delayed(Duration::from_secs(1)),
    ]);
    let eval = openrouter_eval(name, &stub);
    let live = Live::start(&eval, &args_with(&C002_OPUS[..6], &["--repetitions", "3"]));
    stub.next(); // the handler is installed before the first request goes out
    live.signal(signal);
    let (code, err) = live.finish(Duration::from_secs(10));
    assert_eq!(code, Some(130), "{err}");
    assert!(
        err.contains("cancelling: waiting for 1 in-flight trial; press Ctrl-C again to stop now"),
        "{err}"
    );
    assert_eq!(err.matches("cancelling:").count(), 1, "{err}");
    let trials = eval.trials();
    assert_eq!(
        trials.len(),
        1,
        "the in-flight trial is recorded, no new one starts"
    );
    assert_eq!(trials[0]["attempts"][0]["disposition"], "answered");
    assert_eq!(eval.end()["state"], "partial");
    let shown = run(&["show", eval.file().to_str().unwrap()]);
    assert_eq!(shown.status.code(), Some(0), "{}", stderr(&shown));
    assert!(
        stdout(&shown).contains("state: partial (cancelled; not comparable to a complete run)\ntrials: 1 recorded of 3 planned\n"),
        "{}",
        stdout(&shown)
    );
}

#[test]
fn a_cancelled_trial_still_makes_its_retries() {
    let stub = scripted(vec![
        Reply::new(503, r#"{"error":{"message":"busy"}}"#),
        Reply::new(200, OPENROUTER_REPLY),
    ]);
    let eval = openrouter_eval("sigint-retry", &stub);
    let live = Live::start(&eval, &args_with(&C002_OPUS[..6], &["--repetitions", "3"]));
    stub.next();
    live.sigint(); // lands in or before the 1 s retry pause
    let (code, err) = live.finish(Duration::from_secs(10));
    assert_eq!(code, Some(130), "{err}");
    let trials = eval.trials();
    assert_eq!(trials.len(), 1);
    assert_eq!(trials[0]["attempts"].as_array().unwrap().len(), 2);
    assert_eq!(trials[0]["attempts"][1]["disposition"], "answered");
}

#[test]
fn a_second_sigint_exits_130_at_once_and_leaves_no_end_line() {
    let stub = scripted(vec![
        Reply::new(200, OPENROUTER_REPLY).delayed(Duration::from_secs(5)),
    ]);
    let eval = openrouter_eval("sigint-twice", &stub);
    let live = Live::start(&eval, &args_with(&C002_OPUS[..6], &["--repetitions", "3"]));
    stub.next();
    live.sigint();
    live.wait_for_stderr("cancelling:");
    let clock = std::time::Instant::now();
    live.sigint();
    let (code, err) = live.finish(Duration::from_secs(3));
    assert_eq!(code, Some(130), "{err}");
    assert!(
        clock.elapsed() < Duration::from_secs(3),
        "{:?}",
        clock.elapsed()
    );
    let lines = eval.lines();
    assert_eq!(lines.len(), 1, "only the run line");
    let shown = run(&["show", eval.file().to_str().unwrap()]);
    assert!(
        stdout(&shown).contains(
            "state: partial (interrupted: no end record; not comparable to a complete run)\n"
        ),
        "{}",
        stdout(&shown)
    );
}

#[test]
fn a_sigint_during_the_last_trial_still_ends_complete() {
    let stub = scripted(vec![
        Reply::new(200, OPENROUTER_REPLY).delayed(Duration::from_secs(1)),
    ]);
    let eval = openrouter_eval("sigint-last", &stub);
    let live = Live::start(&eval, &C002_OPUS);
    stub.next();
    live.sigint();
    let (code, err) = live.finish(Duration::from_secs(10));
    // The state follows recorded == planned, not the cancel flag.
    assert_eq!(code, Some(0), "{err}");
    assert!(err.contains("cancelling:"), "{err}");
    assert_eq!(eval.end()["state"], "complete");
}

// ---- retries ----

#[test]
fn a_transient_status_is_retried_and_final_attempts_decide_the_counts() {
    let stub = scripted(vec![
        Reply::new(503, r#"{"error":{"message":"busy"}}"#),
        Reply::new(200, OPENROUTER_REPLY),
    ]);
    let eval = openrouter_eval("retry-503", &stub);
    let out = eval.run(
        &args_with(&C002_OPUS[..6], &["--repetitions", "2"]),
        &[("OPENROUTER_API_KEY", TEST_KEY)],
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(stub.pending(), 3);
    let trials = eval.trials();
    assert_eq!(trials[0]["attempts"].as_array().unwrap().len(), 2);
    assert_eq!(trials[0]["attempts"][0]["http_status"], 503);
    assert_eq!(trials[0]["attempts"][1]["disposition"], "answered");
    assert_eq!(trials[1]["attempts"].as_array().unwrap().len(), 1);
    assert!(
        stderr(&out).contains("#1: opus hit (2 attempts, "),
        "{}",
        stderr(&out)
    );
    // The failed first attempt does not count against the trial.
    assert!(
        stdout(&out).contains(
            "  outcomes: 2 answered, 0 provider_error, 0 transport_error; attempts: 3, retried trials: 1"
        ),
        "{}",
        stdout(&out)
    );
}

#[test]
fn a_transport_error_is_retried() {
    let stub = scripted(vec![Reply::dropped(), Reply::new(200, OPENROUTER_REPLY)]);
    let eval = openrouter_eval("retry-transport", &stub);
    let out = eval.run(&C002_OPUS, &[("OPENROUTER_API_KEY", TEST_KEY)]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let attempts = eval.trials()[0]["attempts"].clone();
    assert_eq!(attempts.as_array().unwrap().len(), 2);
    assert_eq!(attempts[0]["disposition"], "transport_error");
    assert_eq!(attempts[1]["disposition"], "answered");
}

#[test]
fn non_transient_statuses_and_answered_replies_are_never_retried() {
    for (n, status, body) in [
        (0, 400, r#"{"error":{"message":"bad"}}"#.to_string()),
        (1, 200, reply(r#""High""#)),
        (2, 200, "{}".to_string()),
    ] {
        let stub = stub(status, &body);
        let eval = openrouter_eval(&format!("no-retry-{n}"), &stub);
        let out = eval.run(&C002_OPUS, &[("OPENROUTER_API_KEY", TEST_KEY)]);
        let expected = if status == 400 { 3 } else { 0 };
        assert_eq!(
            out.status.code(),
            Some(expected),
            "{status}: {}",
            stderr(&out)
        );
        assert_eq!(eval.trials()[0]["attempts"].as_array().unwrap().len(), 1);
        assert_eq!(stub.pending(), 1, "{status} {body}");
    }
}

#[test]
fn persistent_failure_stops_after_one_plus_retries_attempts() {
    let stub = stub(503, r#"{"error":{"message":"busy"}}"#);
    let eval = openrouter_eval("retry-all-503", &stub);
    let out = eval.run(
        &args_with(&C002_OPUS, &["--retries", "2"]),
        &[("OPENROUTER_API_KEY", TEST_KEY)],
    );
    assert_eq!(out.status.code(), Some(3), "{}", stderr(&out));
    assert_eq!(eval.trials()[0]["attempts"].as_array().unwrap().len(), 3);
    assert_eq!(stub.pending(), 3);
    assert!(
        stdout(&out).contains(
            "  outcomes: 0 answered, 1 provider_error, 0 transport_error; attempts: 3, retried trials: 1"
        ),
        "{}",
        stdout(&out)
    );
    assert!(stderr(&out).contains("(3 attempts, "), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("up to 3 requests"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn retries_zero_means_a_single_attempt() {
    let stub = stub(503, "busy");
    let eval = openrouter_eval("retries-0", &stub);
    let out = eval.run(
        &args_with(&C002_OPUS, &["--retries", "0"]),
        &[("OPENROUTER_API_KEY", TEST_KEY)],
    );
    assert_eq!(out.status.code(), Some(3), "{}", stderr(&out));
    assert_eq!(eval.trials()[0]["attempts"].as_array().unwrap().len(), 1);
    assert_eq!(stub.pending(), 1);
    assert_eq!(eval.manifest()["settings"]["retries"], 0);
}

// ---- reading evidence back ----

#[test]
fn a_file_without_an_end_line_is_reported_as_interrupted() {
    let stub = stub(200, OPENROUTER_REPLY);
    let eval = openrouter_eval("no-end", &stub);
    let args = args_with(&C002_OPUS[..6], &["--repetitions", "2"]);
    eval.run(&args, &[("OPENROUTER_API_KEY", TEST_KEY)]);
    let mut lines = eval.lines();
    assert_eq!(lines.pop().unwrap()["type"], "end");
    let cut = temp_file("no-end-evidence.jsonl", &join_lines(&lines));
    let shown = run(&["show", cut.to_str().unwrap()]);
    assert_eq!(shown.status.code(), Some(0), "{}", stderr(&shown));
    let printed = stdout(&shown);
    assert!(
        printed.contains("state: partial (interrupted: no end record; not comparable to a complete run)\ntrials: 2 recorded of 2 planned\n"),
        "{printed}"
    );
}

#[test]
fn a_truncated_last_line_is_tolerated_with_a_warning() {
    let stub = stub(200, OPENROUTER_REPLY);
    let eval = openrouter_eval("truncated-line", &stub);
    let args = args_with(&C002_OPUS[..6], &["--repetitions", "2"]);
    eval.run(&args, &[("OPENROUTER_API_KEY", TEST_KEY)]);
    let text = std::fs::read_to_string(eval.file()).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    let half = &lines[2][..lines[2].len() / 2];
    let cut = temp_file(
        "truncated-evidence.jsonl",
        &format!("{}\n{}\n{half}", lines[0], lines[1]),
    );
    let shown = run(&["show", cut.to_str().unwrap()]);
    assert_eq!(shown.status.code(), Some(0), "{}", stderr(&shown));
    let printed = stdout(&shown);
    assert!(
        printed.contains("ignoring unreadable last line 3"),
        "{printed}"
    );
    assert!(
        printed.contains("state: partial (interrupted: no end record; not comparable to a complete run)\ntrials: 1 recorded of 2 planned\n"),
        "{printed}"
    );
}

#[test]
fn every_run_gets_its_own_file() {
    let stub = stub(200, OPENROUTER_REPLY);
    let eval = openrouter_eval("two-runs", &stub);
    for _ in 0..2 {
        let out = eval.run(&C002_OPUS, &[("OPENROUTER_API_KEY", TEST_KEY)]);
        assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    }
    let files = files_in(&eval.out_dir);
    assert_eq!(files.len(), 2, "{files:?}");
    for file in files {
        let name = file.file_name().unwrap().to_string_lossy().into_owned();
        let stem = name.strip_suffix(".jsonl").expect(&name);
        let (ms, pid) = stem.split_once('-').expect(&name);
        assert!(
            ms.parse::<u64>().is_ok() && pid.parse::<u32>().is_ok(),
            "{name}"
        );
        assert_eq!(jsonl(&file)[0]["id"], stem);
    }
}

// ---- prices and cost ----

#[test]
fn cost_uses_the_price_captured_in_the_run_and_counts_failed_attempts_as_unbilled() {
    let stub = scripted(vec![
        Reply::new(503, r#"{"error":{"message":"busy"}}"#),
        Reply::new(200, OPENROUTER_REPLY),
    ]);
    let priced = |input: &str| {
        format!(
            "{}[candidate.price]\ninput_per_million = {input}\noutput_per_million = 0.08\n",
            candidate_toml("jev-openrouter", "openrouter", "typesafe/jev-1.13")
        )
    };
    let eval = Eval::new("price-snapshot", &priced("0.04"), &stub.url);
    let out = eval.run(&C002_OPUS, &[("OPENROUTER_API_KEY", TEST_KEY)]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    // 488 * 0.04 / 1e6 + 54 * 0.08 / 1e6 = 0.00002384
    let cost = "  cost: 0.0000238 USD over 1 priced attempt; 1 failed attempt counted as 0 USD (assumed unbilled)\n";
    assert!(stdout(&out).contains(cost), "{}", stdout(&out));
    assert_eq!(
        eval.manifest()["candidates"][0]["price"],
        json!({"input_per_million": 0.04, "output_per_million": 0.08})
    );

    // A later config edit must not change what an old run cost.
    std::fs::write(&eval.config, priced("400")).unwrap();
    let shown = run(&["show", eval.file().to_str().unwrap()]);
    assert_eq!(shown.status.code(), Some(0), "{}", stderr(&shown));
    assert!(stdout(&shown).contains(cost), "{}", stdout(&shown));
}

#[test]
fn cost_is_unknown_without_a_price() {
    let stub = stub(200, OPENROUTER_REPLY);
    let eval = openrouter_eval("no-price", &stub);
    let out = eval.run(&C002_OPUS, &[("OPENROUTER_API_KEY", TEST_KEY)]);
    assert!(
        stdout(&out).contains("  cost: unknown (no price for 1 attempt)\n"),
        "{}",
        stdout(&out)
    );
    assert!(eval.manifest()["candidates"][0].get("price").is_none());
}

// ---- default paths and the removed url field ----

/// Runs `run` with no `--config`/`--out`; only the listed path variables reach the child.
fn run_default_paths(url: &str, vars: &[(&str, &Path)], empty: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_decisionbench"));
    for name in [
        "OPENCODE_API_KEY",
        "XDG_CONFIG_HOME",
        "XDG_DATA_HOME",
        "HOME",
    ] {
        command.env_remove(name);
    }
    command.env("DECISIONBENCH_TEST_BASE_URL", url);
    for name in empty {
        command.env(name, "");
    }
    for (name, value) in vars {
        command.env(name, value);
    }
    command
        .arg("run")
        .args(ZEN_C002_OPUS)
        .output()
        .expect("binary runs")
}

fn zen_config() -> String {
    candidate_toml("jev-zen", "zen", "jev-1.13")
}

fn write_config(dir: &Path, toml: &str) {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(dir.join("config.toml"), toml).unwrap();
}

#[test]
fn default_paths_follow_the_xdg_variables() {
    let stub = stub(200, OPENROUTER_REPLY);
    let root = fresh_dir("xdg-paths");
    write_config(&root.join("xdgc/decisionbench"), &zen_config());
    let out = run_default_paths(
        &stub.url,
        &[
            ("XDG_CONFIG_HOME", &root.join("xdgc")),
            ("XDG_DATA_HOME", &root.join("xdgd")),
        ],
        &[],
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(files_in(&root.join("xdgd/decisionbench/runs")).len(), 1);
}

#[test]
fn empty_xdg_variables_fall_back_to_home() {
    let stub = stub(200, OPENROUTER_REPLY);
    let root = fresh_dir("home-paths");
    write_config(&root.join("home/.config/decisionbench"), &zen_config());
    let out = run_default_paths(
        &stub.url,
        &[("HOME", &root.join("home"))],
        &["XDG_CONFIG_HOME", "XDG_DATA_HOME"],
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let runs = root.join("home/.local/share/decisionbench/runs");
    assert_eq!(files_in(&runs).len(), 1);
}

#[test]
fn no_xdg_and_no_home_exits_1_without_a_request() {
    let stub = stub(200, OPENROUTER_REPLY);
    let out = run_default_paths(&stub.url, &[], &[]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("pass the path explicitly"),
        "{}",
        stderr(&out)
    );
    assert!(
        stub.requests
            .recv_timeout(Duration::from_millis(500))
            .is_err(),
        "no request was sent"
    );
}

#[test]
fn a_url_field_in_the_config_is_rejected() {
    let eval = Eval::new(
        "url-field",
        &format!(
            "{}url = \"http://127.0.0.1:1\"\n",
            candidate_toml("jev-zen", "zen", "m")
        ),
        "http://127.0.0.1:1",
    );
    let out = eval.run(&ZEN_C002_OPUS, &[]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("unknown field `url`"),
        "{}",
        stderr(&out)
    );
    assert!(files_in(&eval.out_dir).is_empty());
}

// ---- reports: show, compare, history (offline, hand-built evidence) ----

const DATASET_SHA: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const OTHER_DATASET_SHA: &str = "fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210";
const SHA_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const SHA_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

fn manifest_line(
    id: &str,
    created: u64,
    dataset_sha: &str,
    instructions: &[(&str, &str)],
    candidates: &[(&str, &str, &str)],
    targets: &[&str],
    planned: usize,
) -> Value {
    json!({
        "type": "run", "schema_version": 4, "id": id, "created_unix_ms": created,
        "dataset": {"name": "ds.json", "sha256": dataset_sha},
        "instructions": instructions.iter()
            .map(|(n, s)| json!({"name": n, "sha256": s, "text": "t"})).collect::<Vec<_>>(),
        "grading_revision": 2,
        "candidates": candidates.iter()
            .map(|(n, p, m)| json!({"name": n, "provider": p, "model": m, "url": "http://stub"}))
            .collect::<Vec<_>>(),
        "cases": ["c1"], "targets": targets,
        "settings": {"repetitions": 1, "concurrency": 1, "timeout_secs": 60, "retries": 1, "include_unscored": false},
        "planned_trials": planned,
    })
}

fn attempt_line(latency: u64, disposition: &str, efforts: &[(&str, &str)]) -> Value {
    let body = if disposition == "answered" {
        let choices: Vec<(&str, String)> = efforts
            .iter()
            .map(|(t, e)| (*t, format!("\"{e}\"")))
            .collect();
        let choices: Vec<(&str, &str)> = choices.iter().map(|(t, e)| (*t, e.as_str())).collect();
        reply_for(&choices)
    } else {
        "busy".to_string()
    };
    json!({
        "started_unix_ms": 0, "latency_ms": latency, "disposition": disposition,
        "http_status": match disposition {
            "transport_error" => Value::Null,
            "provider_error" => json!(503),
            _ => json!(200),
        },
        "raw_response": body, "error": null,
    })
}

#[allow(clippy::too_many_arguments)]
fn trial_line(
    seq: usize,
    candidate: &str,
    instructions: (&str, &str),
    case: &str,
    repetition: u32,
    targets: &[&str],
    labels: Value,
    attempts: Vec<Value>,
    elapsed: u64,
) -> Value {
    json!({
        "type": "trial", "seq": seq, "candidate": candidate,
        "instructions": {"name": instructions.0, "sha256": instructions.1},
        "case_id": case, "targets": targets, "repetition": repetition, "labels": labels,
        "request": {}, "attempts": attempts, "started_unix_ms": 0, "elapsed_ms": elapsed,
    })
}

fn end_line(state: &str) -> Value {
    json!({"type": "end", "state": state, "finished_unix_ms": 9})
}

fn write_run(file: &str, lines: &[Value]) -> PathBuf {
    temp_file(file, &join_lines(lines))
}

/// One line with runs of spaces collapsed, so assertions ignore table padding.
fn squash(line: &str) -> String {
    line.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn assert_has_line(text: &str, expected: &str) {
    assert!(
        text.lines().any(|l| squash(l) == expected),
        "missing line {expected:?} in:\n{text}"
    );
}

fn show_text(path: &Path) -> String {
    let out = run(&["show", path.to_str().unwrap()]);
    stdout_of_success(out)
}

const NEUTRAL: (&str, &str) = ("neutral", SHA_A);

/// Partial run (no end line, a cut last line) with candidate `alpha` (7 of its 7 trials recorded)
/// and candidate `beta` (none), 9 planned. Targets: haiku, sonnet, opus.
///
/// Labels: L1 = haiku low-medium, sonnet high, opus max. L2 = sonnet low-medium, opus
/// medium-high, haiku unlabelled. L3 = sonnet high only. L4 = no labels.
/// alpha/neutral trials (T = seq):
///   T0 c1 #1 L1 answered 100 ms, elapsed 120: haiku low, sonnet high, opus max
///   T1 c1 #2 L1 answered 200 ms, elapsed 480: haiku high, sonnet high, opus "ultra"
///   T2 c2 #1 L2 answered 300 ms, elapsed 250: haiku high, sonnet medium, opus low
///   T3 c3 #1 L3 answered 400 ms, elapsed 900: sonnet low, opus high (haiku absent)
///   T4 c3 #2 L3 transport_error 50 ms then provider_error 70 ms, elapsed 1100
///   T5 c4 #1 L1 transport_error 30 ms, elapsed 40
///   T6 c5 #1 L4 answered 1000 ms, elapsed 310: low, low, low
fn partial_report_run() -> PathBuf {
    let t = ["haiku", "sonnet", "opus"];
    let l1 = json!({"haiku": "low-medium", "sonnet": "high", "opus": "max"});
    let l2 = json!({"sonnet": "low-medium", "opus": "medium-high"});
    let l3 = json!({"sonnet": "high"});
    let l4 = json!({});
    let answered = |ms, e: &[(&str, &str)]| vec![attempt_line(ms, "answered", e)];
    let trial = |seq, case, rep, labels: &Value, attempts, elapsed| {
        trial_line(
            seq,
            "alpha",
            NEUTRAL,
            case,
            rep,
            &t,
            labels.clone(),
            attempts,
            elapsed,
        )
    };
    let lines = vec![
        manifest_line(
            "R1",
            1000,
            DATASET_SHA,
            &[NEUTRAL],
            &[("alpha", "zen", "m-a"), ("beta", "openrouter", "m-b")],
            &t,
            9,
        ),
        trial(
            0,
            "c1",
            1,
            &l1,
            answered(
                100,
                &[("haiku", "low"), ("sonnet", "high"), ("opus", "max")],
            ),
            120,
        ),
        trial(
            1,
            "c1",
            2,
            &l1,
            answered(
                200,
                &[("haiku", "high"), ("sonnet", "high"), ("opus", "ultra")],
            ),
            480,
        ),
        trial(
            2,
            "c2",
            1,
            &l2,
            answered(
                300,
                &[("haiku", "high"), ("sonnet", "medium"), ("opus", "low")],
            ),
            250,
        ),
        trial(
            3,
            "c3",
            1,
            &l3,
            answered(400, &[("sonnet", "low"), ("opus", "high")]),
            900,
        ),
        trial(
            4,
            "c3",
            2,
            &l3,
            vec![
                attempt_line(50, "transport_error", &[]),
                attempt_line(70, "provider_error", &[]),
            ],
            1100,
        ),
        trial(
            5,
            "c4",
            1,
            &l1,
            vec![attempt_line(30, "transport_error", &[])],
            40,
        ),
        trial(
            6,
            "c5",
            1,
            &l4,
            answered(
                1000,
                &[("haiku", "low"), ("sonnet", "low"), ("opus", "low")],
            ),
            310,
        ),
    ];
    temp_file(
        "report-partial.jsonl",
        &format!("{}{{\"type\":\"trial\",\"seq\"", join_lines(&lines)),
    )
}

#[test]
fn show_prints_the_full_report_for_a_partial_run() {
    let path = partial_report_run();
    let before = std::fs::read(&path).unwrap();
    let out = show_text(&path);
    assert_eq!(std::fs::read(&path).unwrap(), before);

    // Header.
    assert_has_line(&out, "== Run ==");
    assert_has_line(&out, "id: R1");
    assert_has_line(
        &out,
        "state: partial (interrupted: no end record; not comparable to a complete run)",
    );
    assert_has_line(&out, "trials: 7 recorded of 9 planned");
    assert_has_line(&out, "dataset: ds.json sha256 0123456789ab");
    assert_has_line(&out, "grading revision: 2");
    assert_has_line(&out, "schema: 4");

    // Accuracy per target, hand-counted over the 5 answered trials (T0 T1 T2 T3 T6):
    //   haiku:  hit(T0) over(T1) unscored(T2 T3 T6) -> 1 hit of 2 valid scored, n=5
    //   sonnet: hit(T0 T1 T2) under(T3) unscored(T6) -> 3 of 4, n=5
    //   opus:   hit(T0) invalid(T1 "ultra") under(T2) unscored(T3 T6) -> 1 of 2, n=5
    assert_has_line(&out, "candidate: alpha (zen m-a)");
    assert_has_line(&out, "target neutral (aaaaaaaaaaaa)");
    assert_has_line(&out, "haiku 1/2 (50.0%)");
    assert_has_line(&out, "sonnet 3/4 (75.0%)");
    assert_has_line(&out, "opus 1/2 (50.0%)");
    assert_has_line(
        &out,
        "neutral (aaaaaaaaaaaa) haiku: hit 1, under 0, over 1, invalid 0, unscored 3; n=5",
    );
    assert_has_line(
        &out,
        "neutral (aaaaaaaaaaaa) sonnet: hit 3, under 1, over 0, invalid 0, unscored 1; n=5",
    );
    assert_has_line(
        &out,
        "neutral (aaaaaaaaaaaa) opus: hit 1, under 1, over 0, invalid 1, unscored 2; n=5",
    );
    // beta has no trials: zero denominators.
    assert_has_line(&out, "candidate: beta (openrouter m-b)");
    assert_has_line(&out, "haiku n/a (0 valid scored)");
    assert_has_line(
        &out,
        "neutral (aaaaaaaaaaaa) opus: hit 0, under 0, over 0, invalid 0, unscored 0; n=0",
    );

    // Cross-model. Joint hit eligible: T0 (all 3 labelled and valid, all hit) and T2 (sonnet and
    // opus labelled and valid; opus under); T1 has an invalid labelled opus, T3 has one label.
    // -> 1/2. Gap agreement: T0 has 3 non-overlapping pairs, all strictly ordered as labelled
    // (haiku<sonnet, haiku<opus, sonnet<opus) -> 3; T1 only haiku-sonnet is eligible and
    // high = high is not strict -> 0 of 1; T2's sonnet/opus ranges overlap at medium.
    // -> 3/4.
    assert_has_line(
        &out,
        "neutral (aaaaaaaaaaaa): joint hit 1/2 (50.0%); gap agreement 3/4 (75.0%)",
    );
    assert_has_line(
        &out,
        "neutral (aaaaaaaaaaaa): joint hit n/a (0 eligible); gap agreement n/a (0 eligible)",
    );

    // Consistency: only c1 has two answered repetitions. haiku low/high differ (0 of 1);
    // sonnet high/high agree (1 of 1); opus has one valid effort, so no group.
    assert_has_line(&out, "neutral (aaaaaaaaaaaa) haiku: consistent 0/1 (0.0%)");
    assert_has_line(
        &out,
        "neutral (aaaaaaaaaaaa) sonnet: consistent 1/1 (100.0%)",
    );
    assert_has_line(
        &out,
        "neutral (aaaaaaaaaaaa) opus: consistent n/a (0 cases with >=2 valid efforts)",
    );

    // Coverage: 5 answered of 7 recorded. Labelled predictions among answered, per target
    // (valid scored + invalid): haiku 2, sonnet 4, opus 3 = 9, of which 1 invalid.
    assert_has_line(
        &out,
        "neutral (aaaaaaaaaaaa): answered trials 5/7 (71.4%); invalid labelled predictions 1/9 (11.1%)",
    );
    assert_has_line(
        &out,
        "neutral (aaaaaaaaaaaa): answered trials n/a (0 recorded trials); invalid labelled predictions n/a (0 labelled predictions)",
    );

    // Reliability: T4 ended provider_error, T5 transport_error; attempts 4 + 2 + 1 + 1 = 8;
    // one retried trial (T4); failed attempts 2 + 1 = 3.
    assert_has_line(
        &out,
        "outcomes: 1 provider_error, 1 transport_error; attempts: 8, retried trials: 1, failed attempts: 3",
    );

    // Latency over answered only. Attempts 100 200 300 400 1000: median rank ceil(.5*5)=3 -> 300,
    // p95 rank ceil(.95*5)=5 -> 1000. Trial elapsed 120 250 310 480 900: median 310, p95 900.
    assert_has_line(
        &out,
        "per-attempt (answered attempts): median 300 ms, p95 1000 ms (n=5)",
    );
    assert_has_line(
        &out,
        "logical trial elapsed (answered trials): median 310 ms, p95 900 ms (n=5)",
    );
    assert_has_line(&out, "per-attempt (answered attempts): n/a (n=0)");

    // No price: 8 attempts.
    assert_has_line(&out, "cost: unknown (no price for 8 attempts)");
    // The cut last line is a read warning.
    assert!(out.contains("ignoring unreadable last line 9"), "{out}");
}

#[test]
fn the_header_flags_cancelled_and_failed_runs() {
    let t = ["sonnet"];
    let make = |file: &str, state: &str| {
        write_run(
            file,
            &[
                manifest_line(
                    "S1",
                    5,
                    DATASET_SHA,
                    &[NEUTRAL],
                    &[("alpha", "zen", "m")],
                    &t,
                    1,
                ),
                trial_line(
                    0,
                    "alpha",
                    NEUTRAL,
                    "c1",
                    1,
                    &t,
                    json!({"sonnet": "high"}),
                    vec![attempt_line(10, "answered", &[("sonnet", "high")])],
                    11,
                ),
                end_line(state),
            ],
        )
    };
    assert_has_line(
        &show_text(&make("state-complete.jsonl", "complete")),
        "state: complete",
    );
    assert_has_line(
        &show_text(&make("state-partial.jsonl", "partial")),
        "state: partial (cancelled; not comparable to a complete run)",
    );
    assert_has_line(
        &show_text(&make("state-failed.jsonl", "failed")),
        "state: failed (not comparable to a complete run)",
    );
}

#[test]
fn one_instruction_name_with_two_hashes_is_two_columns_and_a_warning() {
    let t = ["sonnet"];
    let labels = json!({"sonnet": "high"});
    let path = write_run(
        "report-two-hashes.jsonl",
        &[
            manifest_line(
                "H1",
                5,
                DATASET_SHA,
                &[("neutral", SHA_A)],
                &[("alpha", "zen", "m")],
                &t,
                2,
            ),
            trial_line(
                0,
                "alpha",
                ("neutral", SHA_A),
                "c1",
                1,
                &t,
                labels.clone(),
                vec![attempt_line(10, "answered", &[("sonnet", "high")])],
                11,
            ),
            trial_line(
                1,
                "alpha",
                ("neutral", SHA_B),
                "c1",
                1,
                &t,
                labels,
                vec![attempt_line(10, "answered", &[("sonnet", "low")])],
                11,
            ),
            end_line("complete"),
        ],
    );
    let out = show_text(&path);
    assert_has_line(&out, "target neutral (aaaaaaaaaaaa) neutral (bbbbbbbbbbbb)");
    assert_has_line(&out, "sonnet 1/1 (100.0%) 0/1 (0.0%)");
    assert_has_line(
        &out,
        "instructions `neutral` appears with more than one hash: aaaaaaaaaaaa, bbbbbbbbbbbb",
    );
}

#[test]
fn a_single_hash_gives_no_warning() {
    let out = show_text(&partial_report_run());
    assert!(!out.contains("more than one hash"), "{out}");
}

#[allow(clippy::too_many_arguments)]
fn compare_run(
    file: &str,
    id: &str,
    created: u64,
    sha: &str,
    hash: &str,
    provider: &str,
    efforts: [&str; 2],
    end: Option<&str>,
) -> PathBuf {
    let t = ["sonnet"];
    let labels = json!({"sonnet": "high"});
    let ins = ("neutral", hash);
    let mut lines = vec![manifest_line(
        id,
        created,
        sha,
        &[ins],
        &[("c1", provider, "m1")],
        &t,
        2,
    )];
    for (i, e) in efforts.iter().enumerate() {
        lines.push(trial_line(
            i,
            "c1",
            ins,
            "c1",
            i as u32 + 1,
            &t,
            labels.clone(),
            vec![attempt_line(10, "answered", &[("sonnet", e)])],
            11,
        ));
    }
    lines.extend(end.map(end_line));
    write_run(file, &lines)
}

#[test]
fn compare_labels_the_same_prompt_in_two_runs_by_run_id() {
    // X: high (hit), low (under) -> 1/2. Y: high, high -> 2/2.
    let x = compare_run(
        "cmp-x.jsonl",
        "X1",
        1,
        DATASET_SHA,
        SHA_A,
        "zen",
        ["high", "low"],
        Some("complete"),
    );
    let y = compare_run(
        "cmp-y.jsonl",
        "Y1",
        2,
        DATASET_SHA,
        SHA_A,
        "zen",
        ["high", "high"],
        Some("complete"),
    );
    let out = stdout_of_success(run(&["compare", x.to_str().unwrap(), y.to_str().unwrap()]));
    assert_has_line(
        &out,
        "no differences found in: dataset, grading, schema, targets, cases, repetitions, unscored inclusion, candidates and routes",
    );
    assert!(!out.contains("Not directly comparable"), "{out}");
    assert_has_line(&out, "Accuracy: hit / valid scored per prompt");
    assert_has_line(&out, "baseline: neutral in X1");
    assert_has_line(&out, "columns: neutral in X1, neutral in Y1 (prompts)");
    assert_has_line(
        &out,
        "candidate target neutral in X1 (baseline) neutral in Y1",
    );
    // Repetition 2 missed in X and hit in Y: one case gained, too few changed cases for a sign test to call it significant.
    assert_has_line(&out, "c1 sonnet 1/2 (50.0%) 2/2 (100.0%) +1 -0 ~");
    assert_has_line(&out, "pooled all 1/2 (50.0%) 2/2 (100.0%) +1 -0 ~");
}

#[test]
fn compare_lists_every_mismatch_before_the_table() {
    let x = compare_run(
        "cmp-base.jsonl",
        "X1",
        1,
        DATASET_SHA,
        SHA_A,
        "zen",
        ["high", "low"],
        Some("complete"),
    );
    // Different dataset, instruction hash and route, and no end line.
    let z = compare_run(
        "cmp-odd.jsonl",
        "Z1",
        3,
        OTHER_DATASET_SHA,
        SHA_B,
        "openrouter",
        ["high", "high"],
        None,
    );
    let out = stdout_of_success(run(&["compare", x.to_str().unwrap(), z.to_str().unwrap()]));
    let head = out.find("Not directly comparable:").expect(&out);
    let table = out
        .find("Accuracy: hit / valid scored per prompt")
        .expect(&out);
    assert!(head < table, "{out}");
    assert!(!out.contains("comparable: same"), "{out}");
    for expected in [
        "Z1: dataset differs from X1 (fedcba987654 vs 0123456789ab)",
        "Z1: candidate `c1` route differs (openrouter m1 vs zen m1)",
        "Z1: partial run (not comparable to a complete run)",
    ] {
        assert!(
            out.lines().any(|l| squash(l) == format!("- {expected}")),
            "missing {expected:?} in:\n{out}"
        );
    }
    // A prompt that differs in content is a column of its own, not a mismatch.
    assert!(!out.contains("instructions"), "{out}");
    assert_has_line(
        &out,
        "candidate target neutral (aaaaaaaaaaaa) (baseline) neutral (bbbbbbbbbbbb)",
    );
    assert_has_line(&out, "c1 sonnet 1/2 (50.0%) 2/2 (100.0%) +1 -0 ~");
}

#[test]
fn compare_flags_different_target_sets_and_needs_two_readable_files() {
    let x = compare_run(
        "cmp-t1.jsonl",
        "X1",
        1,
        DATASET_SHA,
        SHA_A,
        "zen",
        ["high", "low"],
        Some("complete"),
    );
    let mut lines = jsonl(&x);
    lines[0]["id"] = json!("T2");
    lines[0]["targets"] = json!(["sonnet", "opus"]);
    let y = write_run("cmp-t2.jsonl", &lines);
    let out = stdout_of_success(run(&["compare", x.to_str().unwrap(), y.to_str().unwrap()]));
    assert_has_line(
        &out,
        "- T2: target set differs from X1 (sonnet, opus vs sonnet)",
    );
    // opus was not asked in X1: `-`; asked but never answered in T2: n/a.
    assert_has_line(&out, "c1 opus - n/a (0 valid scored)");

    assert_eq!(
        run(&["compare", x.to_str().unwrap()]).status.code(),
        Some(2)
    );
    assert_eq!(run(&["compare"]).status.code(), Some(2));
    let bad = temp_file("cmp-bad.jsonl", "{");
    let out = run(&["compare", x.to_str().unwrap(), bad.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("cmp-bad.jsonl"), "{}", stderr(&out));
    assert!(out.stdout.is_empty());
}

#[test]
fn history_lists_runs_by_created_time_and_unreadable_files_without_aborting() {
    let dir = fresh_dir("history-dir");
    std::fs::create_dir_all(&dir).unwrap();
    let copy = |from: &Path, name: &str| std::fs::copy(from, dir.join(name)).unwrap();
    let late = compare_run(
        "hist-late.jsonl",
        "LATE",
        2000,
        DATASET_SHA,
        SHA_A,
        "zen",
        ["high", "low"],
        Some("complete"),
    );
    let early = compare_run(
        "hist-early.jsonl",
        "EARLY",
        1000,
        OTHER_DATASET_SHA,
        SHA_A,
        "zen",
        ["high", "low"],
        None,
    );
    copy(&late, "a-late.jsonl");
    copy(&early, "z-early.jsonl");
    std::fs::write(dir.join("junk.jsonl"), "{").unwrap();
    std::fs::write(dir.join("notes.txt"), "ignored").unwrap();
    let out = stdout_of_success(run(&["history", dir.to_str().unwrap()]));
    let rows: Vec<String> = out.lines().map(squash).collect();
    // Created order, not file-name order; the unreadable file comes last.
    // EARLY: no end line -> partial, 2 recorded of 2 planned.
    assert_eq!(rows[0], "id created state trials candidates dataset");
    assert_eq!(rows[1], "EARLY 1000 partial 2/2 c1 fedcba987654");
    assert_eq!(rows[2], "LATE 2000 complete 2/2 c1 0123456789ab");
    assert!(rows[3].starts_with("junk.jsonl unreadable: "), "{rows:?}");
    assert_eq!(rows.len(), 4, "{rows:?}");
}

#[test]
fn history_of_an_empty_or_missing_dir_says_so_and_exits_0() {
    let missing = fresh_dir("history-missing");
    let out = stdout_of_success(run(&["history", missing.to_str().unwrap()]));
    assert_eq!(out, format!("no runs in {}\n", missing.display()));
    std::fs::create_dir_all(&missing).unwrap();
    let out = stdout_of_success(run(&["history", missing.to_str().unwrap()]));
    assert_eq!(out, format!("no runs in {}\n", missing.display()));
}

#[test]
fn history_defaults_to_the_evidence_directory() {
    let data = fresh_dir("history-xdg");
    let runs = data.join("decisionbench/runs");
    std::fs::create_dir_all(&runs).unwrap();
    let x = compare_run(
        "hist-default.jsonl",
        "D1",
        7,
        DATASET_SHA,
        SHA_A,
        "zen",
        ["high", "low"],
        Some("complete"),
    );
    std::fs::copy(&x, runs.join("D1.jsonl")).unwrap();
    let out = stdout_of_success(run_with_env(
        &["history"],
        &[("XDG_DATA_HOME", data.to_str().unwrap())],
    ));
    assert!(
        out.lines()
            .any(|l| squash(l) == "D1 7 complete 2/2 c1 0123456789ab"),
        "{out}"
    );
}

fn variant_of(base: &Path, file: &str, edit: impl FnOnce(&mut Vec<Value>)) -> PathBuf {
    let mut lines = jsonl(base);
    lines[0]["id"] = json!("V2");
    edit(&mut lines);
    write_run(file, &lines)
}

fn compare_pair(a: &Path, b: &Path) -> String {
    stdout_of_success(run(&["compare", a.to_str().unwrap(), b.to_str().unwrap()]))
}

fn compare_base(file: &str) -> PathBuf {
    compare_run(
        file,
        "X1",
        1,
        DATASET_SHA,
        SHA_A,
        "zen",
        ["high", "low"],
        Some("complete"),
    )
}

#[test]
fn compare_flags_different_case_selection() {
    let x = compare_base("cmp-cases-x.jsonl");
    let y = variant_of(&x, "cmp-cases-y.jsonl", |l| {
        l[0]["cases"] = json!(["c1", "c2"]);
    });
    let out = compare_pair(&x, &y);
    assert_has_line(
        &out,
        "- V2: case selection differs from X1 (2 vs 1 cases; only in V2: c2)",
    );
    assert!(!out.contains("no differences found"), "{out}");
}

#[test]
fn compare_caps_listed_case_ids_at_ten() {
    let x = compare_base("cmp-cases-cap-x.jsonl");
    let y = variant_of(&x, "cmp-cases-cap-y.jsonl", |l| {
        let ids: Vec<String> = (0..25).map(|i| format!("d{i:02}")).collect();
        l[0]["cases"] = json!(ids);
    });
    let out = compare_pair(&x, &y);
    assert_has_line(
        &out,
        "- V2: case selection differs from X1 (25 vs 1 cases; only in V2: d00, d01, d02, d03, d04, d05, d06, d07, d08, d09, … +15 more; only in X1: c1)",
    );
}

#[test]
fn compare_flags_different_repetitions_and_unscored_inclusion() {
    let x = compare_base("cmp-reps-x.jsonl");
    let y = variant_of(&x, "cmp-reps-y.jsonl", |l| {
        l[0]["settings"]["repetitions"] = json!(3);
        l[0]["settings"]["include_unscored"] = json!(true);
    });
    let out = compare_pair(&x, &y);
    assert_has_line(&out, "- V2: repetitions differs from X1 (3 vs 1)");
    assert_has_line(
        &out,
        "- V2: unscored inclusion differs from X1 (true vs false)",
    );
}

#[test]
fn compare_flags_candidates_missing_from_either_run() {
    let x = compare_base("cmp-cand-x.jsonl");
    let y = variant_of(&x, "cmp-cand-y.jsonl", |l| {
        l[0]["candidates"].as_array_mut().unwrap().push(json!(
            {"name": "c2", "provider": "zen", "model": "m2", "url": "http://stub"}
        ));
    });
    // One prompt and two candidates: auto makes the candidates the columns, so this is no difference.
    assert!(!compare_pair(&x, &y).contains("is not in"));
    let prompts = |a: &Path, b: &Path| {
        let (a, b) = (a.to_str().unwrap(), b.to_str().unwrap());
        stdout_of_success(run(&["compare", a, b, "--columns", "prompt"]))
    };
    assert_has_line(&prompts(&x, &y), "- V2: candidate `c2` is not in X1");
    assert_has_line(&prompts(&y, &x), "- V2: candidate `c2` is not in X1");
}

#[test]
fn compare_puts_different_prompts_in_columns_with_a_paired_delta() {
    let x = compare_base("cmp-ins-x.jsonl");
    let y = compare_run(
        "cmp-ins-y0.jsonl",
        "Y1",
        2,
        DATASET_SHA,
        SHA_B,
        "zen",
        ["high", "high"],
        Some("complete"),
    );
    let y = variant_of(&y, "cmp-ins-y.jsonl", |l| {
        l[0]["instructions"][0]["name"] = json!("effortless");
        for t in &mut l[1..3] {
            t["instructions"]["name"] = json!("effortless");
        }
    });
    let out = compare_pair(&x, &y);
    assert!(!out.contains("Not directly comparable"), "{out}");
    assert_has_line(
        &out,
        "no differences found in: dataset, grading, schema, targets, cases, repetitions, unscored inclusion, candidates and routes",
    );
    assert_has_line(&out, "baseline: neutral");
    assert_has_line(&out, "columns: neutral, effortless (prompts)");
    assert_has_line(&out, "candidate target neutral (baseline) effortless");
    assert_has_line(&out, "c1 sonnet 1/2 (50.0%) 2/2 (100.0%) +1 -0 ~");
    assert_has_line(
        &out,
        "+g -l = paired trials (case x repetition) gained and lost against the baseline; ~ = not significant (exact sign test on the cases that changed, p >= 0.05)",
    );
}

#[test]
fn compare_pairs_only_the_cases_both_runs_have() {
    let x = compare_base("cmp-pair-x.jsonl");
    let y = compare_run(
        "cmp-pair-y0.jsonl",
        "Y1",
        2,
        DATASET_SHA,
        SHA_A,
        "zen",
        ["high", "high"],
        Some("complete"),
    );
    let y = variant_of(&y, "cmp-pair-y.jsonl", |l| {
        l[0]["cases"] = json!(["c1", "c2"]);
        let extra = trial_line(
            2,
            "c1",
            NEUTRAL,
            "c2",
            1,
            &["sonnet"],
            json!({"sonnet": "high"}),
            vec![attempt_line(10, "answered", &[("sonnet", "low")])],
            11,
        );
        l.insert(3, extra);
    });
    let out = compare_pair(&x, &y);
    assert_has_line(
        &out,
        "- V2: case selection differs from X1 (2 vs 1 cases; only in V2: c2)",
    );
    // V2 has three predictions (2 hits), but only the two shared cases pair: one gained.
    assert_has_line(&out, "c1 sonnet 1/2 (50.0%) 2/3 (66.7%) +1 -0 ~");
}

#[test]
fn show_summarizes_a_run_with_several_prompts_as_columns() {
    let t = ["sonnet"];
    let labels = json!({"sonnet": "high"});
    let prompts = [("p1", SHA_A), ("p2", SHA_B), ("p3", DATASET_SHA)];
    let efforts = ["low", "high", "low"];
    let mut lines = vec![manifest_line(
        "S1",
        1,
        DATASET_SHA,
        &prompts,
        &[("c1", "zen", "m1")],
        &t,
        3,
    )];
    for (i, (ins, effort)) in prompts.iter().zip(efforts).enumerate() {
        lines.push(trial_line(
            i,
            "c1",
            *ins,
            "c1",
            1,
            &t,
            labels.clone(),
            vec![attempt_line(10, "answered", &[("sonnet", effort)])],
            11,
        ));
    }
    lines.push(end_line("complete"));
    let out = show_text(&write_run("show-prompts.jsonl", &lines));
    let (run, summary, accuracy) = (
        out.find("== Run ==").expect(&out),
        out.find("== Summary ==").expect(&out),
        out.find("== Accuracy").expect(&out),
    );
    assert!(run < summary && summary < accuracy, "{out}");
    assert_has_line(&out, "baseline: p1");
    assert_has_line(&out, "columns: p1, p2, p3 (prompts)");
    assert_has_line(&out, "candidate target p1 (baseline) p2 p3");
    assert_has_line(
        &out,
        "c1 sonnet 0/1 (0.0%) 1/1 (100.0%) +1 -0 ~ 0/1 (0.0%) +0 -0 ~",
    );
    // One row: pooling it would repeat the row.
    assert!(!out.contains("pooled"), "{out}");
}

/// Candidates `a` and `b` on prompts `p1` and `p2`, one case, target sonnet (label high).
/// `a` answers `a_p1` on p1 and high on p2; `b` answers high on p1 and low on p2.
fn crossed_run(file: &str, id: &str, prompts: &[(&str, &str)], a_p1: &str) -> PathBuf {
    let t = ["sonnet"];
    let labels = json!({"sonnet": "high"});
    let answers: Vec<_> = [
        ("a", 0, a_p1),
        ("a", 1, "high"),
        ("b", 0, "high"),
        ("b", 1, "low"),
    ]
    .into_iter()
    .filter(|(_, ins, _)| *ins < prompts.len())
    .collect();
    let mut lines = vec![manifest_line(
        id,
        1,
        DATASET_SHA,
        prompts,
        &[("a", "zen", "m1"), ("b", "zen", "m2")],
        &t,
        answers.len(),
    )];
    for (seq, (candidate, ins, effort)) in answers.into_iter().enumerate() {
        lines.push(trial_line(
            seq,
            candidate,
            prompts[ins],
            "c1",
            1,
            &t,
            labels.clone(),
            vec![attempt_line(10, "answered", &[("sonnet", effort)])],
            11,
        ));
    }
    lines.push(end_line("complete"));
    write_run(file, &lines)
}

const TWO_PROMPTS: [(&str, &str); 2] = [("p1", SHA_A), ("p2", SHA_B)];

#[test]
fn show_columns_candidate_puts_candidates_in_columns_and_prompts_in_rows() {
    let file = crossed_run("cols-show.jsonl", "K1", &TWO_PROMPTS, "low");
    let out = stdout_of_success(run(&[
        "show",
        file.to_str().unwrap(),
        "--columns",
        "candidate",
    ]));
    assert_has_line(&out, "baseline: a");
    assert_has_line(&out, "columns: a, b (candidates)");
    assert_has_line(&out, "prompt target a (baseline) b");
    // b against a on the same prompt: gained on p1, lost on p2.
    assert_has_line(&out, "p1 sonnet 0/1 (0.0%) 1/1 (100.0%) +1 -0 ~");
    assert_has_line(&out, "p2 sonnet 1/1 (100.0%) 0/1 (0.0%) +0 -1 ~");
    assert_has_line(&out, "pooled all 1/2 (50.0%) 1/2 (50.0%) +1 -1 ~");
    assert_has_line(
        &out,
        "pooled = rows present in every column, all prompts and targets together, over valid scored predictions; it is not a score",
    );
    // The rest of the report is unchanged by the columns.
    assert!(
        out.contains("== Run ==") && out.contains("== Accuracy"),
        "{out}"
    );
}

#[test]
fn show_picks_prompt_columns_for_several_prompts_and_candidate_columns_for_one_prompt() {
    let file = crossed_run("cols-auto.jsonl", "K1", &TWO_PROMPTS, "low");
    let auto = show_text(&file);
    assert_has_line(&auto, "columns: p1, p2 (prompts)");
    assert_has_line(&auto, "candidate target p1 (baseline) p2");
    assert_eq!(
        auto,
        stdout_of_success(run(&["show", file.to_str().unwrap(), "--columns", "auto"]))
    );
    let prompt = stdout_of_success(run(&[
        "show",
        file.to_str().unwrap(),
        "--columns",
        "prompt",
    ]));
    assert_eq!(auto, prompt);

    // One prompt, two candidates: the candidates are what differs.
    let file = crossed_run("cols-auto-one.jsonl", "K2", &TWO_PROMPTS[..1], "low");
    let auto = show_text(&file);
    assert_has_line(&auto, "columns: a, b (candidates)");
    assert_has_line(&auto, "p1 sonnet 0/1 (0.0%) 1/1 (100.0%) +1 -0 ~");
    // Asking for prompts anyway gives the one column.
    let prompt = stdout_of_success(run(&[
        "show",
        file.to_str().unwrap(),
        "--columns",
        "prompt",
    ]));
    assert_has_line(&prompt, "columns: p1 (prompts)");
}

#[test]
fn compare_columns_candidate_puts_every_runs_candidates_side_by_side() {
    let x = crossed_run("cols-cmp-x.jsonl", "K1", &TWO_PROMPTS, "low");
    let y = crossed_run("cols-cmp-y.jsonl", "K2", &TWO_PROMPTS, "high");
    let (x, y) = (x.to_str().unwrap(), y.to_str().unwrap());
    let out = stdout_of_success(run(&["compare", x, y, "--columns", "candidate"]));
    assert_has_line(&out, "Accuracy: hit / valid scored per candidate");
    assert_has_line(&out, "baseline: a in K1");
    assert_has_line(
        &out,
        "columns: a in K1, b in K1, a in K2, b in K2 (candidates)",
    );
    assert_has_line(
        &out,
        "prompt target a in K1 (baseline) b in K1 a in K2 b in K2",
    );
    assert_has_line(
        &out,
        "p1 sonnet 0/1 (0.0%) 1/1 (100.0%) +1 -0 ~ 1/1 (100.0%) +1 -0 ~ 1/1 (100.0%) +1 -0 ~",
    );
    assert_has_line(
        &out,
        "p2 sonnet 1/1 (100.0%) 0/1 (0.0%) +0 -1 ~ 1/1 (100.0%) +0 -0 ~ 0/1 (0.0%) +0 -1 ~",
    );
    // Without the flag two prompts mean prompt columns, as before.
    let auto = stdout_of_success(run(&["compare", x, y]));
    assert_has_line(&auto, "Accuracy: hit / valid scored per prompt");
    assert_has_line(
        &auto,
        "columns: p1 in K1, p2 in K1, p1 in K2, p2 in K2 (prompts)",
    );
}

/// One prompt `p1`, one case, target sonnet (label high); each candidate answers as given.
fn one_prompt_run(file: &str, id: &str, answers: &[(&str, &str)]) -> PathBuf {
    let t = ["sonnet"];
    let candidates: Vec<_> = answers.iter().map(|(c, _)| (*c, "zen", "m1")).collect();
    let mut lines = vec![manifest_line(
        id,
        1,
        DATASET_SHA,
        &TWO_PROMPTS[..1],
        &candidates,
        &t,
        answers.len(),
    )];
    for (seq, (candidate, effort)) in answers.iter().enumerate() {
        lines.push(trial_line(
            seq,
            candidate,
            TWO_PROMPTS[0],
            "c1",
            1,
            &t,
            json!({"sonnet": "high"}),
            vec![attempt_line(10, "answered", &[("sonnet", effort)])],
            11,
        ));
    }
    lines.push(end_line("complete"));
    write_run(file, &lines)
}

#[test]
fn compare_with_candidates_as_columns_does_not_flag_candidates_missing_from_a_run() {
    // One prompt, different candidates per run: auto uses the candidates as the columns.
    let x = one_prompt_run("cols-cand-x.jsonl", "K1", &[("a", "high"), ("b", "low")]);
    let y = one_prompt_run("cols-cand-y.jsonl", "K2", &[("b", "high"), ("c", "high")]);
    let (x, y) = (x.to_str().unwrap(), y.to_str().unwrap());
    let out = stdout_of_success(run(&["compare", x, y]));
    assert!(!out.contains("is not in"), "{out}");
    assert!(!out.contains("Not directly comparable"), "{out}");
    assert_has_line(
        &out,
        "no differences found in: dataset, grading, schema, targets, cases, repetitions, unscored inclusion and routes",
    );
    assert_has_line(&out, "columns: a, b in K1, b in K2, c (candidates)");
    // b in K2 is the shared candidate: compared with the baseline `a` on the same prompt.
    assert_has_line(
        &out,
        "p1 sonnet 1/1 (100.0%) 0/1 (0.0%) +0 -1 ~ 1/1 (100.0%) +0 -0 ~ 1/1 (100.0%) +0 -0 ~",
    );
    // Asking for prompts as the columns makes the missing candidates a difference again.
    let out = stdout_of_success(run(&["compare", x, y, "--columns", "prompt"]));
    assert_has_line(&out, "- K1: candidate `a` is not in K2");
    assert_has_line(&out, "- K2: candidate `c` is not in K1");
}

#[test]
fn columns_rejects_a_value_that_is_not_an_axis() {
    let file = crossed_run("cols-bad.jsonl", "K1", &TWO_PROMPTS, "low");
    let f = file.to_str().unwrap();
    for args in [
        vec!["show", f, "--columns", "target"],
        vec!["compare", f, f, "--columns", "target"],
    ] {
        let out = run(&args);
        assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
        let err = stderr(&out);
        assert!(
            err.contains("invalid value 'target'") && err.contains("candidate"),
            "{err}"
        );
        assert!(out.stdout.is_empty());
    }
}

#[test]
fn compare_reports_reliability_settings_apart_from_comparability() {
    let x = compare_base("cmp-set-x.jsonl");
    let y = variant_of(&x, "cmp-set-y.jsonl", |l| {
        l[0]["settings"]["retries"] = json!(2);
        l[0]["settings"]["timeout_secs"] = json!(5);
        l[0]["settings"]["concurrency"] = json!(4);
    });
    let out = compare_pair(&x, &y);
    assert_has_line(
        &out,
        "no differences found in: dataset, grading, schema, targets, cases, repetitions, unscored inclusion, candidates and routes",
    );
    assert_has_line(
        &out,
        "settings differ (affect reliability and latency, not accuracy):",
    );
    for line in [
        "- V2: timeout 5s vs 60s",
        "- V2: concurrency 4 vs 1",
        "- V2: retries 2 vs 1",
    ] {
        assert_has_line(&out, line);
    }
    assert!(!out.contains("Not directly comparable"), "{out}");
    assert!(!compare_pair(&x, &x).contains("settings differ"));
}

#[test]
fn tui_without_a_terminal_exits_with_a_clear_error() {
    // crossterm falls back to /dev/tty when stdin is not a tty, so `setsid` drops the controlling
    // terminal: the test can never take over the terminal it runs in.
    if Command::new("setsid").arg("--version").output().is_err() {
        eprintln!("skipped: `setsid` is not installed, so the test cannot rule out a real tty");
        return;
    }
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("tui-no-terminal");
    let out = Command::new("setsid")
        .arg("--wait")
        .arg(env!("CARGO_BIN_EXE_decisionbench"))
        .args(["tui", "--config"])
        .arg(dir.join("config.toml"))
        .arg("--out")
        .arg(dir.join("runs"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("setsid runs");
    assert_eq!(out.status.code(), Some(1), "stderr: {}", stderr(&out));
    assert!(
        stderr(&out).contains("cannot open the terminal"),
        "stderr: {}",
        stderr(&out)
    );
}

#[test]
fn a_priced_candidate_whose_reply_has_no_usage_counts_cost_as_unknown() {
    let stub = stub(200, &reply("\"low\""));
    let config = format!(
        "{}[candidate.price]\ninput_per_million = 0.04\noutput_per_million = 0.08\n",
        candidate_toml("jev-openrouter", "openrouter", "typesafe/jev-1.13")
    );
    let eval = Eval::new("no-usage", &config, &stub.url);
    let out = eval.run(&C002_OPUS, &[("OPENROUTER_API_KEY", TEST_KEY)]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert!(
        stdout(&out).contains("  cost: 1 attempt unknown (no usage)\n"),
        "{}",
        stdout(&out)
    );
}

#[test]
fn a_sonnet_and_opus_run_asks_only_those_and_grades_each_from_its_own_label() {
    let stub = stub(
        200,
        &reply_for(&[("sonnet", r#""low""#), ("opus", r#""high""#)]),
    );
    let dataset = temp_file(
        "sonnet-opus-dataset.json",
        &dataset(
            r#"{"id":"p1","message":"m","ok":{"haiku":"low","sonnet":"low","opus":"medium","fable":"max"}}"#,
        ),
    );
    let eval = openrouter_eval("sonnet-opus", &stub);
    let out = eval.run(
        &[
            "--candidate",
            "jev-openrouter",
            "--repetitions",
            "1",
            "--target",
            "sonnet",
            "--target",
            "opus",
            "--dataset",
            dataset.to_str().unwrap(),
        ],
        &[("OPENROUTER_API_KEY", TEST_KEY)],
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let body = stub.next().body;
    let questions: Vec<&String> = body["questions"].as_object().unwrap().keys().collect();
    assert_eq!(questions, ["effort_sonnet", "effort_opus"]);
    assert!(
        stderr(&out).contains("#1: sonnet hit · opus over ("),
        "{}",
        stderr(&out)
    );
    assert_eq!(eval.trials()[0]["targets"], json!(["sonnet", "opus"]));
}
