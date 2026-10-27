// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::process::Output;

use serde_json::Value;
use serde_json::json;
use sha2::Digest;
use sha2::Sha256;
use tempfile::TempDir;

struct EvidenceRun {
    project: TempDir,
    binaries: TempDir,
    reports: PathBuf,
}

fn invoke(project: &Path, binaries: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_cpp-infra-ci"))
        .arg("--project")
        .arg(project)
        .args(args)
        .env("CPP_INFRA_BIN_DIR", binaries)
        .output()
        .expect("invoke real CI CLI")
}

fn read_json(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).expect("read generated evidence")).expect("parse generated evidence")
}

fn write_json(path: &Path, value: &Value) {
    fs::write(
        path,
        serde_json::to_vec_pretty(value).expect("serialize fixture evidence"),
    )
    .expect("write fixture evidence");
}

fn successful_json(output: &Output) -> Value {
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    serde_json::from_slice(&output.stdout).expect("CLI JSON output")
}

fn run_fixture(pages: bool) -> EvidenceRun {
    let project = tempfile::tempdir().expect("isolated project");
    let binaries = tempfile::tempdir().expect("isolated protocol executables");
    for tool in ["ci", "style", "tools", "pages"] {
        fs::create_dir_all(project.path().join(format!(".infra/{tool}"))).expect("fixture config directory");
        if tool != "ci" {
            fs::write(project.path().join(format!(".infra/{tool}/config.toml")), "schema=1\n")
                .expect("fixture tool config");
        }
    }
    fs::write(project.path().join(".infra/project.toml"), "schema=1\nkind='library'\nsource_roots=[]\ntest_roots=[]\ngenerated_roots=[]\nthird_party_roots=[]\n[profiles.debug]\nconfigure='debug'\nbuild='debug'\ntest='debug'\n").expect("fixture project config");
    fs::write(
        project.path().join(".infra/ci/config.toml"),
        format!(
            "schema=1\ntasks={}\nprimary_profile='debug'\n",
            if pages { "['style','pages']" } else { "['style']" }
        ),
    )
    .expect("fixture CI config");
    // These processes implement only the public sibling-tool report protocol.
    // They intentionally make no claim to run CMake, LLVM, or cloud services.
    let script = r#"#!/usr/bin/env python3
import datetime, hashlib, json, os, struct, sys
from pathlib import Path
args = sys.argv[1:]
def option(name): return args[args.index(name) + 1]
context = json.loads(Path(option('--run-context')).read_text())
report = {key: context[key] for key in ['schema', 'run_id', 'task_id', 'bundle_id', 'project', 'report_root', 'source_digest', 'config_digest']}
report.update(tool=dict(name=Path(sys.argv[0]).name, version='0.1.0', revision=None), source_revision=None, status='passed', started_at=datetime.datetime.now(datetime.timezone.utc).isoformat(), duration_ms=1, external_versions={}, commands=[], diagnostics=[], artifacts=[])
if report['task_id'] == 'style': report['status'] = os.getenv('CPP_INFRA_PROTOCOL_TEST_STATUS', 'passed')
if report['task_id'] == 'pages':
    site = Path(context['report_root']) / 'pages' / 'protocol-call' / 'site'
    site.mkdir(parents=True)
    (site / 'index.html').write_bytes(b'protocol fixture site')
    name, data = b'index.html', b'protocol fixture site'
    sha = hashlib.sha256(struct.pack('<Q', len(name)) + name + struct.pack('<Q', len(data)) + data).hexdigest()
    report['artifacts'] = [dict(kind='site', path='pages/protocol-call/site', sha256=sha, required=True)]
print('protocol fixture executed ' + context['task_id'])
Path(option('--report')).write_text(json.dumps(report))
"#;
    for tool in ["tools", "style", "pages"] {
        let executable = binaries.path().join(format!("cpp-infra-{tool}"));
        fs::write(&executable, script).expect("protocol executable fixture");
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).expect("executable permissions");
    }
    let output = invoke(project.path(), binaries.path(), &["check"]);
    let result = successful_json(&output);
    let reports = PathBuf::from(result["report_root"].as_str().expect("public report root"));
    EvidenceRun {
        project,
        binaries,
        reports,
    }
}

impl EvidenceRun {
    fn cli(&self, args: &[&str]) -> Output {
        invoke(self.project.path(), self.binaries.path(), args)
    }

    fn summarize(&self, phase: &str) -> Output {
        self.cli(&[
            "summarize",
            "--input-dir",
            self.reports.to_str().expect("report path"),
            "--phase",
            phase,
        ])
    }

    fn resolve(&self, task: &str, kind: &str) -> Output {
        self.cli(&[
            "artifact-path",
            "--manifest",
            self.reports.join("run-manifest.json").to_str().expect("manifest"),
            "--input-dir",
            self.reports.to_str().expect("report path"),
            "--task-id",
            task,
            "--kind",
            kind,
        ])
    }

    fn replace_child(&self, task: &str, child: &Value) {
        let child_path = self.reports.join(format!("{task}-tool.json"));
        write_json(&child_path, child);
        let path = self.reports.join(format!("{task}.json"));
        let mut wrapper = read_json(&path);
        let artifacts = wrapper["artifacts"].as_array_mut().expect("wrapper artifacts");
        let reference = artifacts
            .iter_mut()
            .find(|entry| entry["kind"] == "tool-report")
            .expect("original child reference");
        reference["sha256"] = json!(format!(
            "{:x}",
            Sha256::digest(fs::read(child_path).expect("changed child bytes"))
        ));
        write_json(&path, &wrapper);
    }
}

fn expect_protocol_error(output: &Output, diagnostic: &str) {
    assert_eq!(
        output.status.code(),
        Some(2),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains(diagnostic),
        "expected {diagnostic}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn test_summarize_rejects_each_missing_public_report_field() {
    let run = run_fixture(false);
    let path = run.reports.join("style.json");
    let original = read_json(&path);
    for field in [
        "tool",
        "project",
        "report_root",
        "started_at",
        "duration_ms",
        "external_versions",
        "commands",
        "diagnostics",
        "artifacts",
    ] {
        let mut corrupted = original.clone();
        corrupted.as_object_mut().expect("report object").remove(field);
        write_json(&path, &corrupted);
        expect_protocol_error(&run.summarize("final"), &format!("missing {field}"));
    }
}

#[test]
fn test_summarize_rejects_conflicting_portable_identity_and_matrix() {
    let run = run_fixture(false);
    let path = run.reports.join("style.json");
    let original = read_json(&path);
    for field in [
        "schema",
        "task_id",
        "run_id",
        "bundle_id",
        "source_digest",
        "config_digest",
        "source_revision",
    ] {
        let mut corrupted = original.clone();
        corrupted[field] = json!("unrelated-run-evidence");
        write_json(&path, &corrupted);
        expect_protocol_error(&run.summarize("final"), "report identity mismatch: style");
    }
    let mut corrupted = original;
    corrupted["data"]["matrix_digest"] = json!("different-matrix");
    write_json(&path, &corrupted);
    expect_protocol_error(&run.summarize("final"), "different matrix definition for style");
}

#[test]
fn test_summarize_rejects_unknown_status_and_incomplete_tool_identity() {
    let run = run_fixture(false);
    let path = run.reports.join("style.json");
    let original = read_json(&path);
    for status in [json!("success"), json!(null), json!(42)] {
        let mut corrupted = original.clone();
        corrupted["status"] = status;
        write_json(&path, &corrupted);
        expect_protocol_error(&run.summarize("final"), "invalid report status: style");
    }
    for field in ["name", "version", "revision"] {
        let mut corrupted = original.clone();
        corrupted["tool"].as_object_mut().expect("tool object").remove(field);
        write_json(&path, &corrupted);
        expect_protocol_error(&run.summarize("final"), "invalid report tool identity: style");
    }
}

#[test]
fn test_passed_wrapper_requires_a_successful_recorded_command() {
    let run = run_fixture(false);
    let path = run.reports.join("style.json");
    let original = read_json(&path);
    for commands in [json!([]), json!([{"exit_code":1}]), json!(null)] {
        let mut corrupted = original.clone();
        corrupted["commands"] = commands;
        write_json(&path, &corrupted);
        expect_protocol_error(
            &run.summarize("final"),
            "passed task has no successful recorded command: style",
        );
    }
}

#[test]
fn test_wrapper_requires_one_mandatory_original_report() {
    let run = run_fixture(false);
    let path = run.reports.join("style.json");
    let original = read_json(&path);
    for duplicate in [false, true] {
        let mut corrupted = original.clone();
        let artifacts = corrupted["artifacts"].as_array_mut().expect("artifacts");
        let reference = artifacts
            .iter()
            .find(|entry| entry["kind"] == "tool-report")
            .expect("child reference")
            .clone();
        if duplicate {
            artifacts.push(reference);
        } else {
            artifacts
                .iter_mut()
                .find(|entry| entry["kind"] == "tool-report")
                .expect("child reference")["required"] = json!(false);
        }
        write_json(&path, &corrupted);
        expect_protocol_error(&run.summarize("final"), "must retain one required original tool report");
    }
}

#[test]
fn test_rehashed_original_child_cannot_contradict_passed_wrapper() {
    let run = run_fixture(false);
    let original = read_json(&run.reports.join("style-tool.json"));
    for status in ["failed", "error", "cancelled", "skipped", "blocked"] {
        let mut child = original.clone();
        child["status"] = json!(status);
        run.replace_child("style", &child);
        expect_protocol_error(
            &run.summarize("final"),
            "passed wrapper contradicts original child result",
        );
    }
}

#[test]
fn test_required_nonpassed_child_statuses_fail_both_public_summary_phases() {
    let run = run_fixture(false);
    for status in ["failed", "error", "cancelled", "skipped", "blocked"] {
        let output = Command::new(env!("CARGO_BIN_EXE_cpp-infra-ci"))
            .arg("--project")
            .arg(run.project.path())
            .arg("check")
            .env("CPP_INFRA_BIN_DIR", run.binaries.path())
            .env("CPP_INFRA_PROTOCOL_TEST_STATUS", status)
            .output()
            .expect("invoke child status protocol scenario");
        assert_eq!(output.status.code(), Some(1), "required status {status}");
        let summary: Value = serde_json::from_slice(&output.stdout).expect("failed run summary");
        let directory = Path::new(summary["report_root"].as_str().expect("new run root"));
        assert_eq!(read_json(&directory.join("style-tool.json"))["status"], status);
        for phase in ["quality", "final"] {
            let output = run.cli(&[
                "summarize",
                "--input-dir",
                directory.to_str().expect("run directory"),
                "--phase",
                phase,
            ]);
            assert_eq!(output.status.code(), Some(1), "{phase}: {status}");
            let summary: Value = serde_json::from_slice(&output.stdout).expect("failed public summary");
            assert_eq!(summary["status"], "failed");
            assert_eq!(summary["data"]["complete"], status != "blocked");
            assert!(
                summary["data"]["tasks"]
                    .as_array()
                    .expect("task outcomes")
                    .iter()
                    .any(|task| task["task_id"] == "style" && task["status"] == status && task["required"] == true)
            );
        }
    }
}

#[test]
fn test_quality_phase_excludes_unexecuted_pages_but_final_requires_it() {
    let run = run_fixture(true);
    fs::remove_file(run.reports.join("pages.json")).expect("remove disposable Pages wrapper evidence");
    fs::remove_file(run.reports.join("doctor-pages.json")).expect("remove disposable Pages doctor evidence");
    let quality = successful_json(&run.summarize("quality"));
    assert_eq!(quality["data"]["complete"], true);
    let expected = quality["data"]["expected_task_ids"]
        .as_array()
        .expect("quality task ids");
    assert!(
        !expected
            .iter()
            .any(|id| id == "pages" || id == "doctor-pages" || id == "quality-summary")
    );
    let final_output = run.summarize("final");
    assert_eq!(final_output.status.code(), Some(1));
    let final_report: Value = serde_json::from_slice(&final_output.stdout).expect("failed final summary");
    assert_eq!(final_report["status"], "failed");
    assert_eq!(final_report["data"]["complete"], false);
    assert!(
        final_report["data"]["tasks"]
            .as_array()
            .expect("final tasks")
            .iter()
            .any(|task| task["task_id"] == "pages" && task["status"] == "missing" && task["required"] == true)
    );
}

#[test]
fn test_resolver_rejects_unknown_task_missing_report_and_absent_kind() {
    let run = run_fixture(true);
    expect_protocol_error(&run.resolve("unknown", "site"), "unknown artifact task");
    expect_protocol_error(
        &run.resolve("pages", "coverage-html"),
        "expected exactly one matching tool artifact",
    );
    fs::remove_file(run.reports.join("pages.json")).expect("remove disposable Pages report");
    expect_protocol_error(&run.resolve("pages", "site"), "artifact task report missing");
}

#[test]
fn test_resolver_rejects_ambiguous_site_even_when_both_hashes_are_valid() {
    let run = run_fixture(true);
    let mut child = read_json(&run.reports.join("pages-tool.json"));
    let site = child["artifacts"][0].clone();
    child["artifacts"].as_array_mut().expect("site artifacts").push(site);
    run.replace_child("pages", &child);
    expect_protocol_error(
        &run.resolve("pages", "site"),
        "expected exactly one matching tool artifact",
    );
}

#[test]
fn test_resolver_rechecks_private_site_contents_before_returning_path() {
    let run = run_fixture(true);
    fs::write(
        run.reports.join("pages/protocol-call/site/index.html"),
        "modified after Pages success",
    )
    .expect("tamper disposable site");
    expect_protocol_error(&run.resolve("pages", "site"), "artifact path/digest mismatch");
}

#[test]
fn test_resolver_rejects_a_site_alias_even_inside_the_report_root() {
    let run = run_fixture(true);
    std::os::unix::fs::symlink("site", run.reports.join("pages/protocol-call/site-alias"))
        .expect("disposable site alias");
    let mut child = read_json(&run.reports.join("pages-tool.json"));
    child["artifacts"][0]["path"] = json!("pages/protocol-call/site-alias");
    run.replace_child("pages", &child);
    expect_protocol_error(&run.resolve("pages", "site"), "output path contains symbolic link");
}

#[test]
fn test_summary_rejects_symbolic_links_inside_a_directory_artifact() {
    let run = run_fixture(true);
    std::os::unix::fs::symlink("index.html", run.reports.join("pages/protocol-call/site/alias.html"))
        .expect("disposable site content alias");
    expect_protocol_error(&run.summarize("final"), "artifact contains symbolic link");
}

#[test]
fn test_summarize_rejects_escaping_and_malformed_artifact_references() {
    let run = run_fixture(false);
    let path = run.reports.join("style.json");
    let original = read_json(&path);
    for artifact in [
        json!({"kind":"log","path":"../outside.log","sha256":"irrelevant","required":true}),
        json!({"kind":"log","path":42,"required":true}),
    ] {
        let mut corrupted = original.clone();
        corrupted["artifacts"]
            .as_array_mut()
            .expect("artifacts")
            .push(artifact.clone());
        write_json(&path, &corrupted);
        expect_protocol_error(
            &run.summarize("final"),
            if artifact["path"].is_string() {
                "artifact path escapes report root"
            } else {
                "artifact path must be a string"
            },
        );
    }
    let mut corrupted = original;
    corrupted["artifacts"] = json!({});
    write_json(&path, &corrupted);
    expect_protocol_error(&run.summarize("final"), "artifacts must be an array");
}

#[test]
fn test_explicit_required_log_artifact_is_authenticated_by_summary() {
    let run = run_fixture(false);
    let wrapper = read_json(&run.reports.join("style.json"));
    let log = wrapper["commands"][0]["log"]
        .as_str()
        .expect("recorded public command log")
        .to_owned();
    let log_path = run.reports.join(&log);
    let original = fs::read_to_string(&log_path).expect("captured command output");
    assert!(original.contains("protocol fixture executed style"));
    // Only explicitly declared artifacts carry the public SHA256 guarantee.
    let mut child = read_json(&run.reports.join("style-tool.json"));
    child["artifacts"].as_array_mut().expect("child artifacts").push(json!({
        "kind": "log", "path": log, "sha256": format!("{:x}", Sha256::digest(original.as_bytes())), "required": true,
    }));
    run.replace_child("style", &child);
    successful_json(&run.summarize("final"));
    fs::write(log_path, "substituted command output").expect("tamper disposable captured log");
    expect_protocol_error(&run.summarize("final"), "artifact path/digest mismatch");
}

#[test]
fn test_summary_protocol_error_preserves_public_error_report_and_parent_identity() {
    let run = run_fixture(false);
    let path = run.reports.join("style.json");
    let mut corrupted = read_json(&path);
    corrupted["status"] = json!("not-a-protocol-status");
    write_json(&path, &corrupted);
    let context_path = run.reports.join("style-context.json");
    let context = read_json(&context_path);
    for with_context in [false, true] {
        let output_path = run.reports.join(if with_context {
            "parent-error.json"
        } else {
            "direct-error.json"
        });
        let mut args = vec![
            "--report",
            output_path.to_str().expect("error report path"),
            "summarize",
            "--input-dir",
            run.reports.to_str().expect("run root"),
        ];
        if with_context {
            args.extend(["--run-context", context_path.to_str().expect("existing parent context")]);
        }
        expect_protocol_error(&run.cli(&args), "invalid report status: style");
        let report = read_json(&output_path);
        assert_eq!(report["schema"], 1);
        assert_eq!(report["tool"]["name"], "cpp-infra-ci");
        assert_eq!(report["status"], "error");
        assert_eq!(report["diagnostics"][0]["code"], "CI_ERROR");
        assert!(
            report["diagnostics"][0]["message"]
                .as_str()
                .expect("public diagnostic")
                .contains("invalid report status: style")
        );
        if with_context {
            for field in [
                "task_id",
                "run_id",
                "bundle_id",
                "source_digest",
                "config_digest",
                "report_root",
            ] {
                assert_eq!(report[field], context[field], "parent identity field {field}");
            }
        }
    }
}
