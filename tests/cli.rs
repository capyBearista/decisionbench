use std::path::PathBuf;
use std::process::{Command, Output};

const EXPECTED_COUNTS: &str = "\
cases: 71
sonnet labels: 67 (4 unscored)
opus labels: 8 (63 unscored)
";

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_decisionbench"))
        .args(args)
        .output()
        .expect("binary runs")
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
        "null",
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
        {"case":"c001","target":"opus","effort":"medium"},
        {"case":"c002","target":"opus","effort":"max"},
        {"case":"c002","target":"opus"}
    ]"#;
    let stdout = stdout_of_success(grade_files("denominators", predictions, Some(&cases)));
    let expected = "\
c001 sonnet \"high\" hit
c001 sonnet \"low\" under
c001 sonnet \"max\" over
c001 sonnet \"High\" invalid
c001 opus \"medium\" unscored
c002 opus \"max\" hit
c002 opus null invalid
sonnet: 4 predictions: 1 hit, 1 under, 1 over, 1 invalid, 0 unscored; accuracy 1/3 valid scored
opus: 3 predictions: 1 hit, 0 under, 0 over, 1 invalid, 1 unscored; accuracy 1/1 valid scored
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
