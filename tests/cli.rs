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
cases: 71
sonnet labels: 67 (4 unscored)
opus labels: 8 (63 unscored)
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
    ];
    let stdout = assert_accepted("valid.json", &dataset(&cases.join(",")));
    assert!(
        stdout.ends_with("cases: 3\nsonnet labels: 2 (1 unscored)\nopus labels: 2 (1 unscored)\n"),
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
    let bad = VALID.replace(r#""sonnet":"low""#, r#""haiku":"low""#);
    assert_rejected("target.json", &dataset(&bad), "unknown field `haiku`");
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
sonnet: 5 predictions: 2 hit, 1 under, 1 over, 1 invalid, 0 unscored; accuracy 2/4 valid scored
opus: 6 predictions: 1 hit, 0 under, 0 over, 3 invalid, 2 unscored; accuracy 1/1 valid scored
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
sonnet: 1 predictions: 1 hit, 0 under, 0 over, 0 invalid, 0 unscored; accuracy 1/1 valid scored
opus: 0 predictions: 0 hit, 0 under, 0 over, 0 invalid, 0 unscored; accuracy 0/0 valid scored
"
    );
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
            r#"[{"case":"c001","target":"haiku","effort":"low"}]"#,
            "unknown variant `haiku`",
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
const OPENROUTER_REPLY: &str = r#"{"model":"typesafe/jev-1.13-20260917","answers":{"effort":{"type":"choice","choice":"low","probabilities":{"low":0.83,"medium":0.17,"high":0,"xhigh":0,"max":0},"confidence":0.79}},"usage":{"input_tokens":488,"output_tokens":54,"cost":2.0496e-05},"provider":"TypeSafe"}"#;
const ZEN_429: &str = r#"{"type":"error","error":{"type":"FreeUsageLimitError","message":"Free usage limit reached."}}"#;

fn reply(choice: &str) -> String {
    format!(r#"{{"answers":{{"effort":{{"type":"choice","choice":{choice}}}}}}}"#)
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

/// Everything `run` printed before its `evidence:` line.
fn summary(out: &Output) -> String {
    stdout(out)
        .lines()
        .filter(|l| !l.starts_with("evidence: "))
        .map(|l| format!("{l}\n"))
        .collect()
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
    assert_eq!(body["state"]["target_model"], "Claude Opus");
    assert_eq!(
        body["state"]["next_message"],
        "rename the variable usr to user in auth.ts"
    );
    assert!(body["state"].get("recent_conversation").is_none());
    let raw = body.to_string();
    assert!(
        !raw.contains("current_effort") && !raw.contains("current_model"),
        "{raw}"
    );
    let effort = &body["questions"]["effort"];
    assert_eq!(effort["type"], "choice");
    assert!(
        effort["instructions"]
            .as_str()
            .unwrap()
            .starts_with("Choose the reasoning effort")
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
            "planned: 1 candidate × 1 instruction set × 1 pair (0 unlabelled skipped) × 1 repetition = 1 trial, up to 2 requests"
        ),
        "{progress}"
    );
    assert!(
        progress.contains("[1/1] jev-openrouter neutral c002 opus #1: hit (1 attempt, "),
        "{progress}"
    );

    let lines = eval.lines();
    assert_eq!(lines.len(), 3);
    let manifest = &lines[0];
    assert_eq!(manifest["type"], "run");
    assert_eq!(manifest["schema_version"], 2);
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
    assert_eq!(trial["target"], "opus");
    assert_eq!(trial["repetition"], 1);
    assert_eq!(trial["label"], "low");
    assert_eq!(trial["instructions"]["name"], "neutral");
    assert!(trial["instructions"].get("text").is_none());
    assert_eq!(trial["request"]["state"]["target_model"], "Claude Opus");
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
fn paired_targets_differ_only_in_target_model() {
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
    let mut bodies = [stub.next().body, stub.next().body];
    assert_eq!(bodies[0]["state"]["target_model"], "Claude Sonnet");
    assert_eq!(bodies[1]["state"]["target_model"], "Claude Opus");
    assert_eq!(
        bodies[0]["state"]["recent_conversation"],
        "user: earlier\nassistant: ok"
    );
    bodies[1]["state"]["target_model"] = json!("Claude Sonnet");
    assert_eq!(bodies[0], bodies[1]);
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
        assert!(stderr(&out).contains(" #1: invalid ("), "{}", stderr(&out));
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
            "[1/1] jev-zen neutral c002 opus #1: provider_error (HTTP 429): Free usage limit reached. (1 attempt, "
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
    let eval = Eval::new(
        "transport",
        &candidate_toml("jev-openrouter", "openrouter", "m"),
        "http://127.0.0.1:1",
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
    assert!(
        stderr(&out).contains("OPENROUTER_API_KEY"),
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
    std::fs::write(prompts.join("x/same.md"), CUSTOM).unwrap();
    std::fs::write(prompts.join("y/same.md"), CUSTOM.replace("low desc", "o")).unwrap();
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
        vec!["run", "--candidate", "c", "--target", "haiku"],
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
fn show_prints_the_same_summary_and_leaves_the_file_unchanged() {
    let stub = stub(200, OPENROUTER_REPLY);
    let eval = openrouter_eval("show", &stub);
    let out = eval.run(&C002_OPUS, &[("OPENROUTER_API_KEY", TEST_KEY)]);
    assert_eq!(out.status.code(), Some(0));
    let file = eval.file();
    let before = std::fs::read(&file).unwrap();

    let shown = run(&["show", file.to_str().unwrap()]);
    assert_eq!(shown.status.code(), Some(0), "{}", stderr(&shown));
    assert_eq!(stdout(&shown), summary(&out));
    assert!(stdout(&shown).starts_with("run: complete\n"));
    assert_eq!(std::fs::read(&file).unwrap(), before);
}

#[test]
fn show_regrades_from_the_stored_label_and_raw_effort() {
    let stub = stub(200, &reply(r#""max""#));
    let eval = openrouter_eval("show-regrade", &stub);
    eval.run(&C002_OPUS, &[("OPENROUTER_API_KEY", TEST_KEY)]);
    let shown = run(&["show", eval.file().to_str().unwrap()]);
    assert!(
        stdout(&shown).contains(": 1 predictions: 0 hit, 0 under, 1 over, 0 invalid, 0 unscored;"),
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

const CUSTOM: &str = "Pick the effort for this exact test.\n\n## low\nlow desc\n## medium\nmedium desc\n## high\nhigh desc\n## xhigh\nxhigh desc\n## max\nmax desc\n";

#[test]
fn custom_instructions_are_sent_and_recorded_by_name_and_hash() {
    let stub = stub(200, OPENROUTER_REPLY);
    let eval = openrouter_eval("custom", &stub);
    let dir = fresh_dir("custom-prompts");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("custom.md");
    std::fs::write(&path, CUSTOM).unwrap();
    let args = args_with(&C002_OPUS, &["--instructions", path.to_str().unwrap()]);
    let out = eval.run(&args, &[("OPENROUTER_API_KEY", TEST_KEY)]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));

    let effort = &stub.next().body["questions"]["effort"];
    assert_eq!(
        effort["instructions"],
        "Pick the effort for this exact test."
    );
    assert_eq!(
        effort["criteria"],
        json!({"low":"low desc","medium":"medium desc","high":"high desc","xhigh":"xhigh desc","max":"max desc"})
    );
    let hash = hex_sha256(CUSTOM);
    let manifest = eval.manifest();
    assert_eq!(manifest["instructions"][0]["name"], "custom");
    assert_eq!(manifest["instructions"][0]["sha256"], hash);
    assert_eq!(manifest["instructions"][0]["text"], CUSTOM);
    let trial = &eval.trials()[0];
    assert_eq!(
        trial["instructions"],
        json!({"name": "custom", "sha256": hash})
    );
    assert!(stdout(&out).contains(&format!("  custom ({}) opus:", &hash[..12])));
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
    let path = temp_file("bad-instructions.md", "Intro.\n\n## low\nonly low\n");
    let args = args_with(&C002_OPUS, &["--instructions", path.to_str().unwrap()]);
    let out = eval.run(&args, &[("OPENROUTER_API_KEY", TEST_KEY)]);
    assert_eq!(out.status.code(), Some(1));
    let err = stderr(&out);
    assert!(
        err.contains("bad-instructions.md") && err.contains("missing `## medium`"),
        "{err}"
    );
    assert!(files_in(&eval.out_dir).is_empty());
    assert!(stub.requests.try_recv().is_err());
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
            let mut buf = [0; 4096];
            let _ = stream.read(&mut buf);
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
            let mut buf = [0; 4096];
            let _ = stream.read(&mut buf);
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
    for (n, text) in [CUSTOM.to_string(), CUSTOM.replace("low desc", "other desc")]
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

/// `k1` and `k2` are labelled for both targets; `u1` only for sonnet.
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
    std::fs::write(&one, CUSTOM).unwrap();
    std::fs::write(&two, CUSTOM.replace("low desc", "other desc")).unwrap();
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
            "planned: 2 candidates × 2 instruction sets × 4 pairs (0 unlabelled skipped) × 3 repetitions = 48 trials, up to 96 requests"
        ),
        "{}",
        stderr(&out)
    );
    assert_eq!(stub.pending(), 48);
    assert_eq!(eval.manifest()["planned_trials"], 48);
    assert!(stdout(&out).contains("trials: 48 recorded of 48 planned"));

    let mut trials = eval.trials();
    assert_eq!(trials.len(), 48);
    trials.sort_by_key(|t| t["seq"].as_u64().unwrap());
    let key = |t: &Value| {
        format!(
            "{} {} {} {} {}",
            t["candidate"], t["instructions"]["name"], t["case_id"], t["target"], t["repetition"]
        )
    };
    let distinct: std::collections::HashSet<String> = trials.iter().map(key).collect();
    assert_eq!(distinct.len(), 48);
    for (i, t) in trials.iter().enumerate() {
        assert_eq!(t["seq"], i);
        // Repetition 1 of every combination, then repetition 2, then 3.
        assert_eq!(t["repetition"], i / 16 + 1, "{}", key(t));
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
fn unlabelled_pairs_are_skipped_by_default_and_counted() {
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
            "planned: 1 candidate × 1 instruction set × 3 pairs (1 unlabelled skipped) × 1 repetition = 3 trials, up to 6 requests"
        ),
        "{}",
        stderr(&out)
    );
    let trials = eval.trials();
    assert_eq!(trials.len(), 3);
    assert!(trials.iter().all(|t| !t["label"].is_null()));
    assert_eq!(stub.pending(), 3);
}

#[test]
fn include_unscored_adds_the_unlabelled_pairs() {
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
        stderr(&out).contains("4 pairs (0 unlabelled skipped) × 1 repetition = 4 trials"),
        "{}",
        stderr(&out)
    );
    let trials = eval.trials();
    assert_eq!(trials.len(), 4);
    let unlabelled: Vec<&Value> = trials.iter().filter(|t| t["label"].is_null()).collect();
    assert_eq!(unlabelled.len(), 1);
    assert_eq!(unlabelled[0]["case_id"], "u1");
    assert_eq!(unlabelled[0]["target"], "opus");
    assert_eq!(eval.manifest()["settings"]["include_unscored"], true);
    assert!(
        stdout(&out)
            .contains("opus: 2 predictions: 1 hit, 0 under, 0 over, 0 invalid, 1 unscored;"),
        "{}",
        stdout(&out)
    );
}

#[test]
fn selecting_only_unlabelled_pairs_is_an_error_mentioning_include_unscored() {
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
    for (n, concurrency, expected_peak) in [(0, "2", 2), (1, "1", 1)] {
        let stub = scripted(vec![
            Reply::new(200, OPENROUTER_REPLY).delayed(Duration::from_millis(400)),
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

    fn sigint(&self) {
        let pid = self.child.id().to_string();
        let sent = Command::new("kill").args(["-INT", &pid]).status().unwrap();
        assert!(sent.success());
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
    let stub = scripted(vec![
        Reply::new(200, OPENROUTER_REPLY).delayed(Duration::from_secs(1)),
    ]);
    let eval = openrouter_eval("sigint-once", &stub);
    let live = Live::start(&eval, &args_with(&C002_OPUS[..6], &["--repetitions", "3"]));
    stub.next(); // the handler is installed before the first request goes out
    live.sigint();
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
        stdout(&shown).starts_with("run: partial\ntrials: 1 recorded of 3 planned\n"),
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
        stdout(&shown).starts_with("run: partial (interrupted: no end record)\n"),
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
        stderr(&out).contains("#1: hit (2 attempts, "),
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
        printed.starts_with(
            "run: partial (interrupted: no end record)\ntrials: 2 recorded of 2 planned\n"
        ),
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
    assert!(
        stderr(&shown).contains("warning") && stderr(&shown).contains("line 3"),
        "{}",
        stderr(&shown)
    );
    let printed = stdout(&shown);
    assert!(
        printed.starts_with(
            "run: partial (interrupted: no end record)\ntrials: 1 recorded of 2 planned\n"
        ),
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
    let cost = "  cost: 0.0000238 USD over 1 answered attempt; 1 failed attempt counted as 0 USD (assumed unbilled)\n";
    assert!(stdout(&out).contains(cost), "{}", stdout(&out));
    assert_eq!(
        eval.manifest()["candidates"][0]["price"],
        json!({"input_per_million": 0.04, "output_per_million": 0.08, "per_request": 0.0})
    );

    // A later config edit must not change what an old run cost.
    std::fs::write(&eval.config, priced("400")).unwrap();
    let shown = run(&["show", eval.file().to_str().unwrap()]);
    assert_eq!(shown.status.code(), Some(0), "{}", stderr(&shown));
    assert_eq!(stdout(&shown), summary(&out));
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
