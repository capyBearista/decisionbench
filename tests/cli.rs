use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::mpsc::{Receiver, channel};
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
    let mut command = Command::new(env!("CARGO_BIN_EXE_decisionbench"));
    for name in [
        "OPENROUTER_API_KEY",
        "OPENCODE_API_KEY",
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
    command.output().expect("binary runs")
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

// ---- evaluate and show ----

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

struct Stub {
    url: String,
    requests: Receiver<String>,
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
}

/// Serves every connection with the canned reply, reporting each raw request over a channel.
fn stub(status: u16, body: &str) -> Stub {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/decisions", listener.local_addr().unwrap());
    let (sender, requests) = channel();
    let response = format!(
        "HTTP/1.1 {status} Stub\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );
    std::thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            let mut raw = Vec::new();
            let mut chunk = [0; 4096];
            let head_end = loop {
                let n = stream.read(&mut chunk).unwrap();
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
                let n = stream.read(&mut chunk).unwrap();
                raw.extend_from_slice(&chunk[..n]);
            }
            let _ = sender.send(String::from_utf8_lossy(&raw).into_owned());
            let _ = stream.write_all(response.as_bytes());
        }
    });
    Stub { url, requests }
}

fn candidate_toml(name: &str, provider: &str, model: &str, url: &str) -> String {
    format!(
        "[[candidate]]\nname = \"{name}\"\ndecision_model = \"jev-1.13\"\nprovider = \"{provider}\"\nmodel = \"{model}\"\nurl = \"{url}\"\n"
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

/// One `evaluate` run with its own config file and output directory, both named after `test`.
struct Eval {
    out_dir: PathBuf,
    config: PathBuf,
}

impl Eval {
    fn new(test: &str, config_toml: &str) -> Self {
        Self {
            out_dir: fresh_dir(&format!("{test}-out")),
            config: temp_file(&format!("{test}-config.toml"), config_toml),
        }
    }

    fn run(&self, extra: &[&str], envs: &[(&str, &str)]) -> Output {
        let mut args = vec!["evaluate", "--config", self.config.to_str().unwrap()];
        args.extend(["--out", self.out_dir.to_str().unwrap()]);
        args.extend(extra);
        run_with_env(&args, envs)
    }

    /// The only evidence file written so far, parsed.
    fn evidence(&self) -> Value {
        let files = files_in(&self.out_dir);
        assert_eq!(files.len(), 1, "{files:?}");
        serde_json::from_slice(&std::fs::read(&files[0]).unwrap()).unwrap()
    }
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// Everything `evaluate` printed before its `evidence:` line.
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
        &candidate_toml(
            "jev-openrouter",
            "openrouter",
            "typesafe/jev-1.13",
            &stub.url,
        ),
    )
}

const C002_OPUS: [&str; 6] = [
    "--candidate",
    "jev-openrouter",
    "--case",
    "c002",
    "--target",
    "opus",
];

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
fn answered_hit_prints_summary_and_saves_evidence() {
    let stub = stub(200, OPENROUTER_REPLY);
    let eval = openrouter_eval("or-hit", &stub);
    let out = eval.run(&C002_OPUS, &[("OPENROUTER_API_KEY", TEST_KEY)]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));

    let printed = stdout(&out);
    let lines: Vec<&str> = printed.lines().collect();
    assert_eq!(
        lines[0],
        "candidate: jev-openrouter (openrouter typesafe/jev-1.13)"
    );
    assert!(lines[1].starts_with("instructions: neutral ("), "{printed}");
    assert_eq!(lines[2], "case: c002 opus (label low)");
    assert!(
        lines[3].starts_with("attempt: answered, HTTP 200, "),
        "{printed}"
    );
    assert_eq!(lines[4], "effort: \"low\"");
    assert_eq!(lines[5], "grade: hit");
    let files = files_in(&eval.out_dir);
    assert_eq!(lines[6], format!("evidence: {}", files[0].display()));

    let evidence = eval.evidence();
    assert_eq!(evidence["schema_version"], 1);
    assert_eq!(evidence["case_id"], "c002");
    assert_eq!(evidence["target"], "opus");
    assert_eq!(evidence["label"], "low");
    assert_eq!(evidence["candidate"]["url"], stub.url);
    assert_eq!(evidence["candidate"]["provider"], "openrouter");
    assert_eq!(evidence["dataset"]["name"], "bundled cases");
    assert_eq!(evidence["attempt"]["disposition"], "answered");
    assert_eq!(evidence["attempt"]["raw_response"], OPENROUTER_REPLY);
    assert_eq!(evidence["prediction"]["effort"], "low");
    assert_eq!(evidence["prediction"]["response_provider"], "TypeSafe");
    assert!(
        evidence.get("grade").is_none(),
        "the grade is recomputed, not stored"
    );
}

#[test]
fn evidence_never_holds_the_key() {
    let stub = stub(200, OPENROUTER_REPLY);
    let eval = openrouter_eval("or-redact", &stub);
    let out = eval.run(&C002_OPUS, &[("OPENROUTER_API_KEY", TEST_KEY)]);
    assert_eq!(out.status.code(), Some(0));
    let file = std::fs::read_to_string(&files_in(&eval.out_dir)[0]).unwrap();
    assert!(!file.contains(TEST_KEY));
    assert!(!stdout(&out).contains(TEST_KEY) && !stderr(&out).contains(TEST_KEY));
    assert!(!file.to_lowercase().contains("authorization"));
}

#[test]
fn zen_without_a_key_sends_no_authorization_header() {
    let stub = stub(200, OPENROUTER_REPLY);
    let eval = Eval::new(
        "zen-nokey",
        &candidate_toml("jev-zen", "zen", "jev-1.13-free", &stub.url),
    );
    let out = eval.run(
        &[
            "--candidate",
            "jev-zen",
            "--case",
            "c002",
            "--target",
            "opus",
        ],
        &[],
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let request = stub.next();
    assert_eq!(request.header("authorization"), None, "{}", request.head);
    assert_eq!(request.body["model"], "jev-1.13-free");
    assert_eq!(eval.evidence()["candidate"]["provider"], "zen");
}

#[test]
fn zen_with_a_key_sends_it_and_an_empty_key_counts_as_unset() {
    let stub = stub(200, OPENROUTER_REPLY);
    let eval = Eval::new(
        "zen-key",
        &candidate_toml("jev-zen", "zen", "jev-1.13", &stub.url),
    );
    let args = [
        "--candidate",
        "jev-zen",
        "--case",
        "c002",
        "--target",
        "opus",
    ];
    assert_eq!(
        eval.run(&args, &[("OPENCODE_API_KEY", "")]).status.code(),
        Some(0)
    );
    assert_eq!(stub.next().header("authorization"), None);
    assert_eq!(
        eval.run(&args, &[("OPENCODE_API_KEY", "zk")]).status.code(),
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
    let mut bodies = Vec::new();
    for target in ["sonnet", "opus"] {
        let args = [
            "--candidate",
            "jev-openrouter",
            "--case",
            "p1",
            "--target",
            target,
            "--dataset",
            dataset.to_str().unwrap(),
        ];
        let out = eval.run(&args, &[("OPENROUTER_API_KEY", TEST_KEY)]);
        assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
        bodies.push(stub.next().body);
    }
    assert_eq!(bodies[0]["state"]["target_model"], "Claude Sonnet");
    assert_eq!(bodies[1]["state"]["target_model"], "Claude Opus");
    assert_eq!(
        bodies[0]["state"]["recent_conversation"],
        "user: earlier\nassistant: ok"
    );
    bodies[1]["state"]["target_model"] = json!("Claude Sonnet");
    assert_eq!(bodies[0], bodies[1]);
    assert_eq!(files_in(&eval.out_dir).len(), 2, "two runs write two files");
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
            stdout(&out).contains("\ngrade: invalid\n"),
            "{}",
            stdout(&out)
        );
        assert_eq!(eval.evidence()["attempt"]["raw_response"], body);
    }
}

#[test]
fn provider_failure_exits_3_and_keeps_the_raw_body() {
    let stub = stub(429, ZEN_429);
    let eval = Eval::new(
        "zen-429",
        &candidate_toml("jev-zen", "zen", "jev-1.13-free", &stub.url),
    );
    let out = eval.run(
        &[
            "--candidate",
            "jev-zen",
            "--case",
            "c002",
            "--target",
            "opus",
        ],
        &[],
    );
    assert_eq!(out.status.code(), Some(3), "{}", stderr(&out));
    let printed = stdout(&out);
    assert!(
        printed.contains("attempt: provider_error, HTTP 429, "),
        "{printed}"
    );
    assert!(
        printed.contains("error: Free usage limit reached."),
        "{printed}"
    );
    assert!(printed.contains("\ngrade: none\n"), "{printed}");
    let evidence = eval.evidence();
    assert_eq!(evidence["attempt"]["disposition"], "provider_error");
    assert_eq!(evidence["attempt"]["http_status"], 429);
    assert_eq!(evidence["attempt"]["raw_response"], ZEN_429);
    assert_eq!(evidence["prediction"], Value::Null);
}

#[test]
fn transport_failure_exits_3_and_saves_evidence() {
    let eval = Eval::new(
        "transport",
        &candidate_toml("jev-openrouter", "openrouter", "m", "http://127.0.0.1:1"),
    );
    let out = eval.run(&C002_OPUS, &[("OPENROUTER_API_KEY", TEST_KEY)]);
    assert_eq!(out.status.code(), Some(3), "{}", stderr(&out));
    let printed = stdout(&out);
    assert!(printed.contains("attempt: transport_error, "), "{printed}");
    assert!(printed.contains("\nerror: "), "{printed}");
    assert!(printed.contains("\ngrade: none\n"), "{printed}");
    let evidence = eval.evidence();
    assert_eq!(evidence["attempt"]["disposition"], "transport_error");
    assert!(
        evidence["attempt"]["error"]
            .as_str()
            .is_some_and(|e| !e.is_empty())
    );
    assert_eq!(evidence["attempt"]["http_status"], Value::Null);
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
fn input_errors_exit_1_before_any_request() {
    let stub = stub(200, OPENROUTER_REPLY);
    let eval = openrouter_eval("inputs", &stub);
    let unknown_field = Eval::new(
        "inputs-field",
        &format!(
            "{}colour = \"red\"\n",
            candidate_toml("c", "zen", "m", &stub.url)
        ),
    );
    let unknown_provider = Eval::new(
        "inputs-provider",
        &candidate_toml("c", "bing", "m", &stub.url),
    );
    for (eval, args, fragment) in [
        (
            &eval,
            ["--candidate", "nope", "--case", "c002", "--target", "opus"],
            "unknown candidate `nope`",
        ),
        (
            &eval,
            [
                "--candidate",
                "jev-openrouter",
                "--case",
                "c999",
                "--target",
                "opus",
            ],
            "unknown case c999",
        ),
        (
            &unknown_field,
            ["--candidate", "c", "--case", "c002", "--target", "opus"],
            "unknown field `colour`",
        ),
        (
            &unknown_provider,
            ["--candidate", "c", "--case", "c002", "--target", "opus"],
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
        assert!(files_in(&eval.out_dir).is_empty());
    }
    assert!(stub.requests.try_recv().is_err(), "no request was sent");
    let missing = run(&[
        "evaluate",
        "--config",
        "/nonexistent/config.toml",
        "--candidate",
        "c",
        "--case",
        "c002",
        "--target",
        "opus",
    ]);
    assert_eq!(missing.status.code(), Some(1));
    assert!(stderr(&missing).contains("cannot read /nonexistent/config.toml"));
    let bad_target = run(&[
        "evaluate",
        "--candidate",
        "c",
        "--case",
        "c002",
        "--target",
        "haiku",
    ]);
    assert_eq!(bad_target.status.code(), Some(2));
}

#[test]
fn show_prints_the_same_summary_and_leaves_the_file_unchanged() {
    let stub = stub(200, OPENROUTER_REPLY);
    let eval = openrouter_eval("show", &stub);
    let out = eval.run(&C002_OPUS, &[("OPENROUTER_API_KEY", TEST_KEY)]);
    assert_eq!(out.status.code(), Some(0));
    let file = files_in(&eval.out_dir).remove(0);
    let before = std::fs::read(&file).unwrap();

    let shown = run(&["show", file.to_str().unwrap()]);
    assert_eq!(shown.status.code(), Some(0), "{}", stderr(&shown));
    assert_eq!(stdout(&shown), summary(&out));
    assert!(stdout(&shown).ends_with("grade: hit\n"));
    assert_eq!(std::fs::read(&file).unwrap(), before);
}

#[test]
fn show_regrades_from_the_stored_label_and_raw_effort() {
    let stub = stub(200, &reply(r#""max""#));
    let eval = openrouter_eval("show-regrade", &stub);
    eval.run(&C002_OPUS, &[("OPENROUTER_API_KEY", TEST_KEY)]);
    let shown = run(&["show", files_in(&eval.out_dir)[0].to_str().unwrap()]);
    assert!(
        stdout(&shown).contains("effort: \"max\"\ngrade: over\n"),
        "{}",
        stdout(&shown)
    );
}

#[test]
fn show_rejects_unknown_schema_versions_and_bad_files() {
    let stub = stub(200, OPENROUTER_REPLY);
    let eval = openrouter_eval("show-schema", &stub);
    eval.run(&C002_OPUS, &[("OPENROUTER_API_KEY", TEST_KEY)]);
    let mut evidence = eval.evidence();
    evidence["schema_version"] = json!(99);
    let future = temp_file("future-evidence.json", &evidence.to_string());
    let out = run(&["show", future.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("schema_version 99"),
        "{}",
        stderr(&out)
    );
    assert!(out.stdout.is_empty());
    let junk = temp_file("junk-evidence.json", "{");
    assert_eq!(
        run(&["show", junk.to_str().unwrap()]).status.code(),
        Some(1)
    );
    assert_eq!(run(&["show", "/nonexistent/e.json"]).status.code(), Some(1));
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
    let mut args = C002_OPUS.to_vec();
    args.extend(["--instructions", path.to_str().unwrap()]);
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
    let evidence = eval.evidence();
    assert_eq!(evidence["instructions"]["name"], "custom");
    assert_eq!(evidence["instructions"]["sha256"], hash);
    assert_eq!(evidence["instructions"]["text"], CUSTOM);
    assert!(stdout(&out).contains(&format!("instructions: custom ({})", &hash[..12])));
}

#[test]
fn default_instructions_are_recorded_as_neutral() {
    let stub = stub(200, OPENROUTER_REPLY);
    let eval = openrouter_eval("neutral", &stub);
    eval.run(&C002_OPUS, &[("OPENROUTER_API_KEY", TEST_KEY)]);
    let evidence = eval.evidence();
    let bundled = std::fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("prompts/neutral.md"),
    )
    .unwrap();
    assert_eq!(evidence["instructions"]["name"], "neutral");
    assert_eq!(evidence["instructions"]["text"], bundled);
    assert_eq!(evidence["instructions"]["sha256"], hex_sha256(&bundled));
}

#[test]
fn malformed_instructions_exit_1_naming_the_file() {
    let stub = stub(200, OPENROUTER_REPLY);
    let eval = openrouter_eval("bad-instructions", &stub);
    let path = temp_file("bad-instructions.md", "Intro.\n\n## low\nonly low\n");
    let mut args = C002_OPUS.to_vec();
    args.extend(["--instructions", path.to_str().unwrap()]);
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

// ---- review fixes ----

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
    assert_eq!(mode(&files_in(&eval.out_dir)[0]), 0o600);
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
        &candidate_toml("jev-openrouter", "openrouter", "m", &url),
    );
    let out = eval.run(&C002_OPUS, &[("OPENROUTER_API_KEY", TEST_KEY)]);
    assert_eq!(out.status.code(), Some(3), "{}", stderr(&out));
    assert!(
        stdout(&out).contains("attempt: transport_error, HTTP 200, "),
        "{}",
        stdout(&out)
    );
    let attempt = &eval.evidence()["attempt"];
    assert_eq!(attempt["disposition"], "transport_error");
    assert_eq!(attempt["http_status"], 200);
    assert!(attempt["error"].as_str().is_some_and(|e| !e.is_empty()));
}

#[test]
fn a_key_echoed_by_the_provider_is_redacted() {
    let body = format!(r#"{{"error":{{"message":"Invalid key {TEST_KEY}"}}}}"#);
    let stub = stub(401, &body);
    let eval = openrouter_eval("echo-key", &stub);
    let out = eval.run(&C002_OPUS, &[("OPENROUTER_API_KEY", TEST_KEY)]);
    assert_eq!(out.status.code(), Some(3), "{}", stderr(&out));
    let file = std::fs::read_to_string(&files_in(&eval.out_dir)[0]).unwrap();
    assert!(!file.contains(TEST_KEY), "{file}");
    assert!(file.contains("Invalid key [redacted]"), "{file}");
    assert!(!stdout(&out).contains(TEST_KEY) && !stderr(&out).contains(TEST_KEY));
    assert!(
        stdout(&out).contains("error: Invalid key [redacted]"),
        "{}",
        stdout(&out)
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
        let out = eval.run(&C002_OPUS, &[("OPENROUTER_API_KEY", TEST_KEY)]);
        assert_eq!(out.status.code(), Some(3), "{}", stderr(&out));
        assert!(!stdout(&out).contains('\u{1b}'), "{:?}", stdout(&out));
        assert!(stdout(&out).contains("[31mred"), "{:?}", stdout(&out));
        assert_eq!(eval.evidence()["attempt"]["raw_response"], body);
    }
}

#[test]
fn unlabelled_target_prints_unlabelled_and_grades_unscored() {
    let stub = stub(200, OPENROUTER_REPLY);
    let dataset = temp_file(
        "unlabelled-dataset.json",
        &dataset(r#"{"id":"u1","message":"m","ok":{"sonnet":"low"}}"#),
    );
    let eval = openrouter_eval("unlabelled", &stub);
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
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let printed = stdout(&out);
    assert!(printed.contains("case: u1 opus (unlabelled)"), "{printed}");
    assert!(printed.contains("\ngrade: unscored\n"), "{printed}");
    assert_eq!(eval.evidence()["label"], Value::Null);
}

#[test]
fn zen_auth_error_exits_3_with_the_provider_message() {
    let stub = stub(
        401,
        r#"{"type":"error","error":{"type":"AuthError","message":"Missing API key."}}"#,
    );
    let eval = Eval::new(
        "zen-401",
        &candidate_toml("jev-zen", "zen", "jev-1.13", &stub.url),
    );
    let out = eval.run(
        &[
            "--candidate",
            "jev-zen",
            "--case",
            "c002",
            "--target",
            "opus",
        ],
        &[],
    );
    assert_eq!(out.status.code(), Some(3), "{}", stderr(&out));
    assert!(
        stdout(&out).contains("attempt: provider_error, HTTP 401, "),
        "{}",
        stdout(&out)
    );
    assert!(
        stdout(&out).contains("\nerror: Missing API key.\n"),
        "{}",
        stdout(&out)
    );
}

#[test]
fn non_json_error_body_is_shown_cut_to_200_characters() {
    let body = "é".repeat(300);
    let stub = stub(502, &body);
    let eval = openrouter_eval("bad-gateway", &stub);
    let out = eval.run(&C002_OPUS, &[("OPENROUTER_API_KEY", TEST_KEY)]);
    assert_eq!(out.status.code(), Some(3), "{}", stderr(&out));
    let expected = format!("\nerror: {}...\n", "é".repeat(200));
    assert!(stdout(&out).contains(&expected), "{}", stdout(&out));
    assert_eq!(eval.evidence()["attempt"]["raw_response"], body);
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
        let mut args = C002_OPUS.to_vec();
        args.extend(["--instructions", path.to_str().unwrap()]);
        let out = eval.run(&args, &[("OPENROUTER_API_KEY", TEST_KEY)]);
        assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
        let evidence = eval.evidence();
        assert_eq!(evidence["instructions"]["name"], "custom");
        recorded.push(evidence["instructions"]["sha256"].clone());
    }
    assert_ne!(recorded[0], recorded[1]);
}
