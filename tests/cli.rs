use std::path::PathBuf;
use std::process::{Command, Output};

use sha2::{Digest, Sha256};

const PINNED_SHA256: &str = "b94a548fc5ec096fb717a65943a42175d0b1de34b01833e8407ca14ce2f95c4f";

const EXPECTED_SUMMARY: &str = "\
dataset: effortless judge-cases (capyBearista/effortless@99c03f222d9a71dd1e0d2fa5824eb1cb59bbb20c)
cases: 75
sonnet labels: 67
opus labels: 8
holdout: 20 (sonnet 19, opus 1)
categories:
  answer: 14
  big: 8
  follow-up: 5
  meta: 5
  model-relative: 5
  non-code: 7
  normal: 13
  short-but-big: 7
  trivial: 11
";

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_decisionbench"))
        .args(args)
        .output()
        .expect("binary runs")
}

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("datasets/effortless/judge-cases.json")
}

/// Writes `contents` to a uniquely named file in the target temp dir.
fn temp_file(name: &str, contents: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    let path = dir.join(name);
    std::fs::write(&path, contents).unwrap();
    path
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Guards against local drift; equality with upstream is checked via the git blob SHA in SOURCE.md.
#[test]
fn bundled_source_matches_pinned_digest() {
    assert_eq!(
        sha256_hex(decisionbench::dataset::EFFORTLESS_SOURCE),
        PINNED_SHA256
    );
}

#[test]
fn inspect_bundled_dataset_prints_summary() {
    let out = run(&["dataset", "inspect"]);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(String::from_utf8(out.stdout).unwrap(), EXPECTED_SUMMARY);
}

#[test]
fn inspect_explicit_path_prints_same_counts() {
    let path = fixture_path();
    let out = run(&["dataset", "inspect", path.to_str().unwrap()]);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8(out.stdout).unwrap();
    let (first, rest) = stdout.split_once('\n').unwrap();
    assert_eq!(first, format!("dataset: {}", path.display()));
    assert_eq!(rest, EXPECTED_SUMMARY.split_once('\n').unwrap().1);
}

fn case(fields: &str) -> String {
    format!(r#"{{"about": "x", "cases": [{fields}]}}"#)
}

const VALID: &str = r#"{"id":"a","kind":"trivial","current":{"model":"sonnet","effort":"low"},"message":"m","ok":["low"]}"#;

fn assert_rejected(name: &str, contents: &str, expected_fragment: &str) {
    let path = temp_file(name, contents);
    let out = run(&["dataset", "inspect", path.to_str().unwrap()]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "stderr: {stderr}");
    assert!(out.stdout.is_empty(), "no summary on failure");
    assert!(
        stderr.contains(expected_fragment),
        "expected {expected_fragment:?} in stderr: {stderr}"
    );
}

#[test]
fn minimal_valid_file_is_accepted() {
    let path = temp_file("valid.json", &case(VALID));
    let out = run(&["dataset", "inspect", path.to_str().unwrap()]);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn rejects_malformed_json() {
    assert_rejected("malformed.json", "{\"cases\": [", "EOF");
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
fn rejects_empty_dataset() {
    assert_rejected("empty.json", r#"{"about": "x", "cases": []}"#, "no cases");
}

#[test]
fn rejects_missing_required_field() {
    let no_message = VALID.replace(r#","message":"m""#, "");
    assert_rejected(
        "missing.json",
        &case(&no_message),
        "missing field `message`",
    );
}

#[test]
fn rejects_unknown_effort() {
    let bad = VALID.replace(r#""ok":["low"]"#, r#""ok":["max"]"#);
    assert_rejected("effort.json", &case(&bad), "unknown variant `max`");
}

#[test]
fn rejects_unknown_target_model() {
    let bad = VALID.replace(r#""model":"sonnet""#, r#""model":"haiku""#);
    assert_rejected("model.json", &case(&bad), "unknown variant `haiku`");
}

#[test]
fn rejects_empty_acceptable_set() {
    let bad = VALID.replace(r#""ok":["low"]"#, r#""ok":[]"#);
    assert_rejected(
        "empty-ok.json",
        &case(&bad),
        "case a: empty acceptable-effort set",
    );
}

#[test]
fn rejects_duplicate_ids() {
    assert_rejected(
        "dup.json",
        &case(&format!("{VALID},{VALID}")),
        "duplicate case id a",
    );
}

#[test]
fn rejects_unknown_field() {
    let bad = VALID.replace(r#""id":"a""#, r#""id":"a","extra":1"#);
    assert_rejected("unknown.json", &case(&bad), "unknown field `extra`");
}

#[test]
fn rejects_non_boolean_holdout() {
    let bad = VALID.replace(r#""id":"a""#, r#""id":"a","holdout":"yes""#);
    assert_rejected("holdout.json", &case(&bad), "expected a boolean");
}
