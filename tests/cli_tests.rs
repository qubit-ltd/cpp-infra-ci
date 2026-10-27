// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================

use std::fs;
use std::process::Command;
use std::process::Output;

use serde_json::Value;
use tempfile::TempDir;

fn fixture(tasks: &str, matrix: &str) -> TempDir {
    let root = tempfile::tempdir().expect("temporary project");
    for tool in [
        "ci",
        "style",
        "verify",
        "analyze",
        "coverage",
        "dependency",
        "tools",
        "pages",
    ] {
        fs::create_dir_all(root.path().join(format!(".infra/{tool}"))).expect("config directory");
        if tool != "ci" {
            fs::write(
                root.path().join(format!(".infra/{tool}/config.toml")),
                if tool == "coverage" {
                    "schema=1\nprofile='coverage'\n"
                } else {
                    "schema=1\nprofile='analysis'\n"
                },
            )
            .expect("tool config");
        }
    }
    fs::write(root.path().join(".infra/project.toml"), "schema=1\nkind='library'\nsource_roots=[]\ntest_roots=[]\ngenerated_roots=[]\nthird_party_roots=[]\n[profiles.debug]\nconfigure='debug'\nbuild='debug'\ntest='debug'\n[profiles.analysis]\nconfigure='analysis'\nbuild='analysis'\ntest='analysis'\n[profiles.coverage]\nconfigure='coverage'\nbuild='coverage'\ntest='coverage'\n").expect("project config");
    fs::write(
        root.path().join(".infra/ci/config.toml"),
        format!("schema=1\ntasks=[{tasks}]\nprimary_profile='debug'\n{matrix}\n"),
    )
    .expect("CI config");
    root
}
fn invoke(root: &TempDir, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_cpp-infra-ci"))
        .arg("--project")
        .arg(root.path())
        .args(args)
        .output()
        .expect("invoke CLI")
}
#[test]
fn test_only_adds_required_dependencies_and_preserves_plan() {
    let root = fixture("'style','verify','analyze'", "");
    let out = invoke(&root, &["plan", "--only", "analyze"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let plan: Value = serde_json::from_slice(&out.stdout).expect("machine plan");
    let nodes = plan["nodes"].as_array().expect("nodes");
    assert!(nodes.iter().any(|n| n["tool"] == "cpp-infra-tools"));
    assert!(
        nodes
            .iter()
            .any(|n| n["tool"] == "cpp-infra-verify" && n["profile"] == "analysis")
    );
    assert!(!nodes.iter().any(|n| n["tool"] == "cpp-infra-style"));
    assert!(!root.path().join("build").exists(), "plan must be read only");
}
#[test]
fn test_entire_matrix_is_validated_before_selection() {
    let root = fixture(
        "'verify'",
        "[[matrix]]\nid='host'\nrunner='ubuntu-latest'\nos='linux'\narch='x86_64'\nprofile='debug'\nsuites=['test']\n[[matrix]]\nid='broken'\nrunner='ubuntu-latest'\nos='linux'\narch='x86_64'\nprofile='missing'\nsuites=['test']",
    );
    let out = invoke(&root, &["matrix", "run", "--check", "host"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("missing"));
}
#[test]
fn test_rejects_unknown_and_duplicate_only_tasks() {
    let root = fixture("'style'", "");
    for only in ["style,style", "unknown"] {
        assert_eq!(invoke(&root, &["plan", "--only", only]).status.code(), Some(2));
    }
}
#[test]
fn test_missing_reports_cannot_produce_passed_summary() {
    let root = fixture("'style'", "");
    let inputs = root.path().join("reports");
    fs::create_dir(&inputs).expect("inputs");
    let out = invoke(&root, &["summarize", "--input-dir", inputs.to_str().expect("path")]);
    assert_ne!(
        out.status.code(),
        Some(0),
        "an empty directory has no evidence of required tasks"
    );
}
#[test]
fn test_json_version_has_truthful_revision() {
    let root = fixture("'style'", "");
    let out = invoke(&root, &["--version", "--json"]);
    let version: Value = serde_json::from_slice(&out.stdout).expect("version JSON");
    assert_eq!(version["name"], "cpp-infra-ci");
    assert_eq!(version["protocol"], 1);
    assert!(version.get("revision").is_some());
}

#[cfg(unix)]
fn binaries(root: &TempDir) -> TempDir {
    use std::os::unix::fs::PermissionsExt;
    let bin = tempfile::tempdir().expect("fake binary directory");
    let script = r#"#!/usr/bin/env python3
import sys,os,json,datetime
from pathlib import Path
a=sys.argv[1:]
role=Path(sys.argv[0]).name
allowed={'--project','--report','--run-context','--offline'}
if role=='cpp-infra-tools':allowed.update({'--task','--profile'})
if role in ['cpp-infra-verify','cpp-infra-analyze']:allowed.add('--profile')
if role=='cpp-infra-verify':allowed.update({'--suite','--output'})
if role=='cpp-infra-dependency':allowed.add('--context')
for argument in a:
 if argument.startswith('--') and argument not in allowed:
  print('unrecognized argument '+argument,file=sys.stderr);sys.exit(2)
def get(flag): return a[a.index(flag)+1]
c=json.load(open(get('--run-context')))
task=c['task_id']
if os.getenv('CI_FAKE_STRICT_SNAPSHOT'):
 import hashlib
 snapshot=Path(c['runtime_snapshot']);root=Path(c['report_root']).resolve()
 if not snapshot.resolve().is_relative_to(root):
  print('runtime snapshot is outside report_root',file=sys.stderr);sys.exit(2)
 if hashlib.sha256(snapshot.read_bytes()).hexdigest()!=c['runtime_snapshot_digest']:
  print('runtime snapshot digest mismatch',file=sys.stderr);sys.exit(2)
 if os.getenv('CPP_INFRA_RUNTIME_SNAPSHOT')!=str(snapshot):
  print('runtime snapshot environment differs from context',file=sys.stderr);sys.exit(2)
 if os.getenv('CI_FAKE_MUTATE_RUNTIME')==task:snapshot.write_bytes(snapshot.read_bytes()+b'\n')
if os.getenv('CI_FAKE_REQUIRE_QUALITY')==task and not (Path(c['report_root'])/'quality-summary.json').exists():
 print('quality summary must precede Pages preflight',file=sys.stderr);sys.exit(2)
with open(Path(c['project'])/'build'/'calls.log','a') as f: f.write(task+' '+ ' '.join(a)+'\n')
if os.getenv('CI_FAKE_MISSING')==task: sys.exit(int(os.getenv('CI_FAKE_MISSING_EXIT','0')))
if os.getenv('CI_FAKE_SLEEP')==task:
 import time;time.sleep(5)
status='failed' if os.getenv('CI_FAKE_FAIL')==task else 'passed'
r={k:c[k] for k in ['schema','run_id','task_id','bundle_id','project','report_root','source_digest','config_digest']}
r.update(tool={'name':Path(sys.argv[0]).name,'version':'0.1.0','revision':None},source_revision=None,status=status,started_at=datetime.datetime.now(datetime.timezone.utc).isoformat(),duration_ms=1,external_versions={},commands=[],diagnostics=[],artifacts=[])
if os.getenv('CI_FAKE_BAD_DIGEST')==task:r['source_digest']='stale'
if os.getenv('CI_FAKE_SITE')==task:
 import hashlib,struct
 site=Path(c['report_root'])/'pages/call-fixture/site';site.mkdir(parents=True);(site/'index.html').write_text('site')
 name=b'index.html';content=b'site';digest=hashlib.sha256(struct.pack('<Q',len(name))+name+struct.pack('<Q',len(content))+content).hexdigest()
 r['artifacts']=[dict(kind='site',path='pages/call-fixture/site',sha256=digest,required=True)]
if os.getenv('CI_FAKE_ARTIFACT')==task:
 import hashlib
 payload=Path(c['report_root'])/(task+'-data.txt');payload.write_text('original evidence')
 r['artifacts']=[dict(kind='result',path=payload.name,sha256=hashlib.sha256(payload.read_bytes()).hexdigest(),required=True)]
json.dump(r,open(get('--report'),'w'))
if os.getenv('CI_FAKE_MUTATE')==task:(Path(c['project'])/'changed.cpp').write_text('int changed;')
if '--output' in a: json.dump({'schema':1},open(get('--output'),'w'))
sys.exit(int(os.getenv('CI_FAKE_EXIT','1')) if status=='failed' else 0)
"#;
    for tool in ["tools", "style", "verify", "analyze", "coverage", "dependency", "pages"] {
        let path = bin.path().join(format!("cpp-infra-{tool}"));
        fs::write(&path, script).expect("fake tool");
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).expect("executable");
    }
    fs::create_dir_all(root.path().join("build")).expect("build output");
    bin
}
#[cfg(unix)]
fn run_with(root: &TempDir, bin: &TempDir, args: &[&str], environment: &[(&str, &str)]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_cpp-infra-ci"));
    command
        .arg("--project")
        .arg(root.path())
        .args(args)
        .env("CPP_INFRA_BIN_DIR", bin.path());
    for (key, value) in environment {
        command.env(key, value);
    }
    command.output().expect("CLI invocation")
}
#[cfg(unix)]
#[test]
fn test_check_executes_real_processes_and_both_summary_stages() {
    let root = fixture("'style','verify','analyze','coverage','pages'", "");
    let bin = binaries(&root);
    let out = run_with(&root, &bin, &["check"], &[]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let value: Value = serde_json::from_slice(&out.stdout).expect("summary");
    assert_eq!(value["status"], "passed");
    let report_root = std::path::Path::new(value["report_root"].as_str().expect("root"));
    let quality: Value =
        serde_json::from_slice(&fs::read(report_root.join("quality-summary.json")).expect("quality summary"))
            .expect("quality JSON");
    assert_eq!(quality["data"]["complete"], true);
    assert!(
        !quality["data"]["expected_task_ids"]
            .as_array()
            .expect("ids")
            .iter()
            .any(|id| id == "pages")
    );
    let calls = fs::read_to_string(root.path().join("build/calls.log")).expect("process trace");
    assert!(calls.contains("prepare-analysis"));
    assert!(calls.contains("prepare-coverage"));
    assert!(calls.contains("--suite test"));
    assert!(calls.contains(" collect"));
    assert!(!calls.contains("collect --profile"));
    let ctx: Value = serde_json::from_slice(&fs::read(report_root.join("pages-context.json")).expect("pages context"))
        .expect("context JSON");
    assert_eq!(ctx["artifacts"][0]["kind"], "quality-summary");
}
#[cfg(unix)]
#[test]
fn test_child_nonzero_is_propagated_and_fail_fast_nodes_are_blocked() {
    let root = fixture("'style','verify'", "");
    let bin = binaries(&root);
    let out = run_with(
        &root,
        &bin,
        &["check"],
        &[("CI_FAKE_FAIL", "style"), ("CI_FAKE_EXIT", "2")],
    );
    assert_eq!(out.status.code(), Some(2), "{}", String::from_utf8_lossy(&out.stderr));
    let value: Value = serde_json::from_slice(&out.stdout).expect("summary");
    assert_eq!(value["status"], "failed");
    assert!(
        value["data"]["tasks"]
            .as_array()
            .expect("tasks")
            .iter()
            .any(|t| t["status"] == "blocked")
    );
}
#[cfg(unix)]
#[test]
fn test_stale_child_report_is_not_rewritten_into_success() {
    let root = fixture("'style'", "");
    let bin = binaries(&root);
    assert_eq!(
        run_with(&root, &bin, &["check"], &[("CI_FAKE_BAD_DIGEST", "style")])
            .status
            .code(),
        Some(2)
    );
}
#[cfg(unix)]
#[test]
fn test_summarize_detects_deleted_and_duplicate_required_reports() {
    let root = fixture("'style'", "");
    let bin = binaries(&root);
    let out = run_with(&root, &bin, &["check"], &[]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let value: Value = serde_json::from_slice(&out.stdout).expect("summary");
    let dir = std::path::Path::new(value["report_root"].as_str().expect("root"));
    let summary = invoke(&root, &["summarize", "--input-dir", dir.to_str().expect("path")]);
    assert!(summary.status.success(), "{}", String::from_utf8_lossy(&summary.stderr));
    fs::create_dir(dir.join("duplicate")).expect("duplicate directory");
    fs::copy(dir.join("style.json"), dir.join("duplicate/style.json")).expect("duplicate report");
    assert_eq!(
        invoke(&root, &["summarize", "--input-dir", dir.to_str().expect("path")])
            .status
            .code(),
        Some(2)
    );
    fs::remove_file(dir.join("duplicate/style.json")).expect("remove disposable duplicate fixture");
    fs::remove_file(dir.join("style.json")).expect("remove disposable fixture report");
    assert_eq!(
        invoke(&root, &["summarize", "--input-dir", dir.to_str().expect("path")])
            .status
            .code(),
        Some(1)
    );
}
#[test]
fn test_directed_cycle_in_hook_prerequisites_is_rejected() {
    let root = fixture("'style'", "[[hooks]]\nid='hook'\nexecutable='hook'\nneeds=['hook']");
    fs::write(root.path().join("hook"), "").expect("hook fixture");
    let out = invoke(&root, &["plan"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("cycle"));
}

#[cfg(unix)]
#[test]
fn test_timeout_stops_child_and_reports_failure() {
    let root = fixture("'style'", "timeout_seconds=1");
    let bin = binaries(&root);
    let out = run_with(&root, &bin, &["check"], &[("CI_FAKE_SLEEP", "style")]);
    assert_eq!(out.status.code(), Some(1));
    let summary: Value = serde_json::from_slice(&out.stdout).expect("summary");
    let dir = std::path::Path::new(summary["report_root"].as_str().expect("root"));
    let report: Value =
        serde_json::from_slice(&fs::read(dir.join("style.json")).expect("timeout report")).expect("report JSON");
    assert_eq!(report["diagnostics"][0]["code"], "CI_TIMEOUT");
}
#[cfg(unix)]
#[test]
fn test_matrix_failure_keeps_other_profile_branch_running() {
    let root = fixture(
        "'verify'",
        "local_fail_fast=false\ntask_jobs=2\n[[matrix]]\nid='a'\nrunner='ubuntu-latest'\nos='linux'\narch='x86_64'\nprofile='debug'\nsuites=['build','test']\n[[matrix]]\nid='b'\nrunner='ubuntu-latest'\nos='linux'\narch='x86_64'\nprofile='analysis'\nsuites=['test']",
    );
    let bin = binaries(&root);
    let out = run_with(&root, &bin, &["check"], &[("CI_FAKE_FAIL", "job.matrix-a.verify-test")]);
    assert_eq!(out.status.code(), Some(1));
    let calls = fs::read_to_string(root.path().join("build/calls.log")).expect("calls");
    assert!(calls.contains("job.matrix-b.verify-test"));
    assert_eq!(
        calls
            .lines()
            .filter(|s| s.starts_with("job.matrix-a.verify-build "))
            .count(),
        1
    );
    assert_eq!(
        calls.lines().filter(|s| s.starts_with("job.matrix-a.prepare ")).count(),
        1
    );
}
#[cfg(unix)]
#[test]
fn test_sigint_cancels_running_process_group() {
    use std::process::Stdio;
    use std::time::Duration;
    use std::time::Instant;
    let root = fixture("'style'", "");
    let bin = binaries(&root);
    let child = Command::new(env!("CARGO_BIN_EXE_cpp-infra-ci"))
        .arg("--project")
        .arg(root.path())
        .arg("check")
        .env("CPP_INFRA_BIN_DIR", bin.path())
        .env("CI_FAKE_SLEEP", "style")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn CLI");
    let begin = Instant::now();
    loop {
        let log = fs::read_to_string(root.path().join("build/calls.log")).unwrap_or_default();
        if log.lines().any(|s| s.starts_with("style ")) {
            break;
        }
        assert!(begin.elapsed() < Duration::from_secs(10), "style never started");
        std::thread::sleep(Duration::from_millis(10));
    }
    unsafe {
        libc::kill(child.id() as i32, libc::SIGINT);
    }
    let out = child.wait_with_output().expect("cancelled CLI");
    assert_eq!(out.status.code(), Some(130), "{}", String::from_utf8_lossy(&out.stderr));
    let value: Value = serde_json::from_slice(&out.stdout).expect("cancelled summary");
    assert_eq!(value["status"], "failed");
}
#[cfg(unix)]
#[test]
fn test_summary_detects_tampered_required_child_evidence() {
    let root = fixture("'style'", "");
    let bin = binaries(&root);
    let out = run_with(&root, &bin, &["check"], &[]);
    assert!(out.status.success());
    let value: Value = serde_json::from_slice(&out.stdout).expect("summary");
    let dir = std::path::Path::new(value["report_root"].as_str().expect("root"));
    fs::write(dir.join("style-tool.json"), "tampered").expect("tamper fixture evidence");
    assert_eq!(
        invoke(&root, &["summarize", "--input-dir", dir.to_str().expect("root")])
            .status
            .code(),
        Some(2)
    );
}

#[cfg(unix)]
#[test]
fn test_manifest_can_execute_separate_jobs_then_quality_and_final() {
    let root = fixture("'style','verify'", "");
    let bin = binaries(&root);
    let manifest = root.path().join("build/manifest.json");
    assert!(
        invoke(&root, &["plan", "--output", manifest.to_str().expect("path")])
            .status
            .success()
    );
    let identity: Value = serde_json::from_slice(&fs::read(&manifest).expect("manifest")).expect("JSON");
    let output = root
        .path()
        .join("build/infra-runs")
        .join(identity["run_id"].as_str().expect("run id"));
    for tasks in [
        "style",
        "verify-debug-build,verify-debug-test",
        "quality-summary",
        "final-summary",
    ] {
        let out = run_with(
            &root,
            &bin,
            &[
                "execute",
                "--manifest",
                manifest.to_str().expect("manifest"),
                "--task-ids",
                tasks,
                "--output-dir",
                output.to_str().expect("output"),
            ],
            &[],
        );
        assert!(
            out.status.success(),
            "{tasks}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let quality: Value =
        serde_json::from_slice(&fs::read(output.join("quality-summary.json")).expect("quality")).expect("quality JSON");
    assert_eq!(quality["status"], "passed");
}

#[cfg(unix)]
#[test]
fn test_export_preserves_only_owned_task_evidence() {
    let root = fixture("'style','verify'", "");
    let bin = binaries(&root);
    let out = run_with(&root, &bin, &["check"], &[]);
    assert!(out.status.success());
    let value: Value = serde_json::from_slice(&out.stdout).expect("summary");
    let directory = value["report_root"].as_str().expect("root");
    let exported = root.path().join("build/exported");
    let out = invoke(
        &root,
        &[
            "export",
            "--input-dir",
            directory,
            "--task-ids",
            "doctor-style,style",
            "--output-dir",
            exported.to_str().expect("output"),
        ],
    );
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(exported.join("style.json").is_file());
    assert!(exported.join("style-tool.json").is_file());
    assert!(!exported.join("verify-debug-test.json").exists());
}
#[test]
fn test_matrix_plan_assigns_every_external_node_exactly_once() {
    let root = fixture("'style','verify','analyze','coverage','pages'", "");
    let out = invoke(&root, &["matrix", "plan"]);
    assert!(out.status.success());
    let matrix: Value = serde_json::from_slice(&out.stdout).expect("matrix");
    let mut assigned = std::collections::BTreeSet::new();
    for field in ["light_task_ids", "analysis_task_ids", "hook_task_ids", "pages_task_ids"] {
        for id in matrix[field]
            .as_str()
            .expect("task id group")
            .split(',')
            .filter(|s| !s.is_empty())
        {
            assert!(assigned.insert(id.to_owned()), "duplicate ownership {id}");
        }
    }
    for item in matrix["include"].as_array().expect("matrix include") {
        for id in item["task_ids"]
            .as_str()
            .expect("task ids")
            .split(',')
            .filter(|s| !s.is_empty())
        {
            assert!(assigned.insert(id.to_owned()), "duplicate matrix ownership {id}");
        }
    }
    for node in matrix["plan"]["nodes"].as_array().expect("nodes") {
        if node["tool"] != "internal" {
            assert!(
                assigned.contains(node["id"].as_str().expect("id")),
                "missing ownership {}",
                node["id"]
            );
        }
    }
}

#[cfg(unix)]
#[test]
fn test_child_cli_exit_without_report_keeps_original_category_and_log() {
    let root = fixture("'style'", "");
    let bin = binaries(&root);
    let out = run_with(
        &root,
        &bin,
        &["check"],
        &[("CI_FAKE_MISSING", "style"), ("CI_FAKE_MISSING_EXIT", "2")],
    );
    assert_eq!(out.status.code(), Some(2), "{}", String::from_utf8_lossy(&out.stderr));
    let value: Value = serde_json::from_slice(&out.stdout).expect("summary");
    let dir = std::path::Path::new(value["report_root"].as_str().expect("root"));
    let task: Value =
        serde_json::from_slice(&fs::read(dir.join("style.json")).expect("task report")).expect("task JSON");
    assert_eq!(task["commands"][0]["exit_code"], 2);
    assert_eq!(task["diagnostics"][0]["code"], "CI_CHILD_REPORT_MISSING");
}

#[cfg(unix)]
#[test]
fn test_plan_does_not_spawn_git_or_other_external_tools() {
    use std::os::unix::fs::PermissionsExt;
    let root = fixture("'style'", "");
    let bin = tempfile::tempdir().expect("fake PATH");
    fs::create_dir_all(root.path().join("build")).expect("build");
    let marker = root.path().join("build/git-called");
    let program = bin.path().join("git");
    fs::write(
        &program,
        format!("#!/bin/sh\n: > '{}'\nprintf '%040d\\n' 0\n", marker.display()),
    )
    .expect("fake git");
    fs::set_permissions(&program, fs::Permissions::from_mode(0o755)).expect("executable");
    let out = Command::new(env!("CARGO_BIN_EXE_cpp-infra-ci"))
        .arg("--project")
        .arg(root.path())
        .arg("plan")
        .env("PATH", bin.path())
        .output()
        .expect("plan");
    assert!(out.status.success());
    assert!(!marker.exists(), "planning must only inspect local files");
}
#[test]
fn test_remote_matrix_coverage_is_explicit_and_can_be_required() {
    let root = fixture(
        "'verify'",
        "require_complete_matrix=true\n[[matrix]]\nid='remote'\nrunner='windows-latest'\nos='windows'\narch='x86_64'\nprofile='debug'\nsuites=['test']",
    );
    let out = invoke(&root, &["plan"]);
    assert!(out.status.success());
    let plan: Value = serde_json::from_slice(&out.stdout).expect("plan");
    assert_eq!(plan["remote_only"][0], "remote");
    assert_eq!(plan["incomplete_remote_coverage"], true);
    assert_eq!(plan["require_complete_matrix"], true);
}

#[cfg(unix)]
#[test]
fn test_ci_config_symlink_cannot_escape_project() {
    let root = fixture("'style'", "");
    let outside = tempfile::tempdir().expect("outside");
    let config = root.path().join(".infra/ci/config.toml");
    let external = outside.path().join("config.toml");
    fs::copy(&config, &external).expect("outside fixture");
    fs::remove_file(&config).expect("replace disposable config");
    std::os::unix::fs::symlink(external, config).expect("escape symlink");
    assert_eq!(invoke(&root, &["plan"]).status.code(), Some(2));
}

#[cfg(unix)]
#[test]
fn test_summary_verifies_nested_tool_artifacts() {
    let root = fixture("'style'", "");
    let bin = binaries(&root);
    let out = run_with(&root, &bin, &["check"], &[("CI_FAKE_ARTIFACT", "style")]);
    assert!(out.status.success());
    let value: Value = serde_json::from_slice(&out.stdout).expect("summary");
    let dir = std::path::Path::new(value["report_root"].as_str().expect("root"));
    fs::write(dir.join("style-data.txt"), "changed evidence").expect("alter disposable evidence");
    assert_eq!(
        invoke(&root, &["summarize", "--input-dir", dir.to_str().expect("directory")])
            .status
            .code(),
        Some(2)
    );
}

#[cfg(unix)]
#[test]
fn test_pages_preflight_waits_for_quality_summary() {
    let root = fixture("'style','pages'", "");
    let bin = binaries(&root);
    let out = run_with(&root, &bin, &["check"], &[("CI_FAKE_REQUIRE_QUALITY", "doctor-pages")]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
}

#[test]
fn test_only_pages_includes_configured_quality_prerequisites() {
    let root = fixture("'style','verify','pages'", "");
    let out = invoke(&root, &["plan", "--only", "pages"]);
    assert!(out.status.success());
    let plan: Value = serde_json::from_slice(&out.stdout).expect("plan");
    let nodes = plan["nodes"].as_array().expect("nodes");
    assert!(nodes.iter().any(|n| n["id"] == "style"));
    assert!(nodes.iter().any(|n| n["id"] == "verify-debug-test"));
}

#[cfg(unix)]
#[test]
fn test_input_changed_during_child_execution_rejects_passed_report() {
    let root = fixture("'style'", "");
    let bin = binaries(&root);
    let out = run_with(&root, &bin, &["check"], &[("CI_FAKE_MUTATE", "style")]);
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn test_profile_name_cannot_escape_report_paths() {
    let root = fixture("'verify'", "");
    let project = root.path().join(".infra/project.toml");
    let mut text = fs::read_to_string(&project).expect("project");
    text.push_str("\n[profiles.'../escape']\nconfigure='debug'\nbuild='debug'\n");
    fs::write(project, text).expect("project config");
    let path = root.path().join(".infra/ci/config.toml");
    let text = fs::read_to_string(&path)
        .expect("CI")
        .replace("primary_profile='debug'", "primary_profile='../escape'");
    fs::write(path, text).expect("CI config");
    assert_eq!(invoke(&root, &["plan"]).status.code(), Some(2));
}

#[cfg(unix)]
#[test]
fn test_context_cannot_redirect_run_artifacts_outside_project() {
    let root = fixture("'style'", "");
    let bin = binaries(&root);
    let out = invoke(&root, &["plan"]);
    let plan: Value = serde_json::from_slice(&out.stdout).expect("plan");
    let outside = tempfile::tempdir().expect("outside");
    let context = serde_json::json!({"schema":1,"run_id":plan["run_id"],"task_id":"ci","bundle_id":plan["bundle_id"],"project":root.path().canonicalize().expect("root"),"profile":"debug","source_digest":plan["source_digest"],"config_digest":plan["config_digest"],"report_root":outside.path()});
    let path = root.path().join("build/context.json");
    fs::write(&path, serde_json::to_vec(&context).expect("serialize context")).expect("context");
    assert_eq!(
        run_with(
            &root,
            &bin,
            &["--run-context", path.to_str().expect("context"), "check"],
            &[]
        )
        .status
        .code(),
        Some(2)
    );
}

#[cfg(unix)]
#[test]
fn test_root_report_is_excluded_and_forwarded_across_repeated_runs() {
    let root = fixture("'style','verify'", "");
    let bin = binaries(&root);
    let output = root.path().join("result.json");
    let args = ["--report", output.to_str().expect("report"), "check"];
    let first = run_with(&root, &bin, &args, &[]);
    assert!(first.status.success(), "{}", String::from_utf8_lossy(&first.stderr));
    let one: Value = serde_json::from_slice(&fs::read(&output).expect("report")).expect("JSON");
    assert_eq!(one["source_excluded_paths"], serde_json::json!(["result.json"]));
    let second = run_with(&root, &bin, &args, &[]);
    assert!(second.status.success(), "{}", String::from_utf8_lossy(&second.stderr));
    let two: Value = serde_json::from_slice(&fs::read(&output).expect("report")).expect("JSON");
    assert_eq!(one["source_digest"], two["source_digest"]);
    let directory = std::path::Path::new(two["report_root"].as_str().expect("run root"));
    let context: Value =
        serde_json::from_slice(&fs::read(directory.join("prepare-debug-context.json")).expect("child context"))
            .expect("JSON");
    assert_eq!(context["source_excluded_paths"], serde_json::json!(["result.json"]));
}
#[test]
fn test_report_cannot_overwrite_business_json_or_protected_configuration() {
    let root = fixture("'style'", "");
    let business = root.path().join("business.json");
    fs::write(&business, "{\"business\":true}").expect("business file");
    for output in [
        &business,
        &root.path().join("CMakePresets.json"),
        &root.path().join(".infra/result.json"),
    ] {
        assert_eq!(
            invoke(&root, &["--report", output.to_str().expect("path"), "plan"])
                .status
                .code(),
            Some(2)
        );
    }
    assert_eq!(
        fs::read_to_string(business).expect("preserved file"),
        "{\"business\":true}"
    );
}
#[test]
fn test_transaction_exclusion_is_exact_and_nested_build_is_source() {
    let root = fixture("'style'", "");
    let plan = |root: &TempDir| -> Value { serde_json::from_slice(&invoke(root, &["plan"]).stdout).expect("plan") };
    let before = plan(&root);
    for path in [".infra/.tmp/x", ".infra/transactions/x", ".infra/.transactions/x"] {
        let path = root.path().join(path);
        fs::create_dir_all(path.parent().expect("parent")).expect("temporary dir");
        fs::write(path, "temporary").expect("transaction fixture");
    }
    assert_eq!(before["config_digest"], plan(&root)["config_digest"]);
    let path = root.path().join(".infra/.transactions-policy/x");
    fs::create_dir_all(path.parent().expect("parent")).expect("policy dir");
    fs::write(path, "policy input").expect("policy file");
    assert_ne!(before["config_digest"], plan(&root)["config_digest"]);
    let path = root.path().join("src/build/tracked.cpp");
    fs::create_dir_all(path.parent().expect("parent")).expect("source dir");
    fs::write(path, "int tracked;").expect("source file");
    assert_ne!(before["source_digest"], plan(&root)["source_digest"]);
}

#[test]
fn test_review_matrix_preserves_platforms_and_same_runner_preparation() {
    let root = fixture(
        "'verify'",
        "[[matrix]]\nid='linux'\nrunner='ubuntu-latest'\nos='linux'\narch='x86_64'\nprofile='debug'\nsuites=['build']\n[[matrix]]\nid='linux-tests'\nrunner='ubuntu-latest'\nos='linux'\narch='x86_64'\nprofile='debug'\nsuites=['test']\n[[matrix]]\nid='windows'\nrunner='windows-latest'\nos='windows'\narch='x86_64'\nprofile='debug'\nsuites=['build','test']",
    );
    let out = invoke(&root, &["matrix", "plan"]);
    assert!(out.status.success());
    let value: Value = serde_json::from_slice(&out.stdout).expect("matrix");
    let jobs = value["include"].as_array().expect("jobs");
    assert_eq!(jobs.len(), 2, "one complete build job per execution platform/profile");
    assert!(jobs.iter().any(|entry| entry["os"] == "windows"));
    for job in jobs {
        let ids = job["task_ids"].as_str().expect("ids");
        let owned: Vec<_> = value["plan"]["nodes"]
            .as_array()
            .expect("nodes")
            .iter()
            .filter(|node| ids.split(',').any(|id| node["id"] == id))
            .collect();
        assert_eq!(owned.iter().filter(|node| node["args"][0] == "prepare").count(), 1);
        assert_eq!(owned.iter().filter(|node| node["args"][0] == "run").count(), 2);
    }
}
#[cfg(unix)]
#[test]
fn test_review_summary_authenticates_wrapper_and_child_evidence() {
    let root = fixture("'style'", "");
    let bin = binaries(&root);
    let out = run_with(&root, &bin, &["check"], &[]);
    assert!(out.status.success());
    let summary: Value = serde_json::from_slice(&out.stdout).expect("summary");
    let dir = std::path::Path::new(summary["report_root"].as_str().expect("root"));
    let file = dir.join("style.json");
    let original = fs::read(&file).expect("wrapper");
    let original_value: Value = serde_json::from_slice(&original).expect("JSON");
    let original_child = fs::read(dir.join("style-tool.json")).expect("raw child");
    for mutation in [
        "producer",
        "producer-revision",
        "missing-child",
        "child-tool",
        "child-revision",
    ] {
        let mut value = original_value.clone();
        if mutation == "producer" {
            value["tool"] = serde_json::json!({"name":"unrelated-tool","version":"999","revision":"bad"});
        } else if mutation == "producer-revision" {
            value["tool"]["revision"] = serde_json::json!("a".repeat(40));
        } else if mutation == "missing-child" {
            value["artifacts"] = serde_json::json!([]);
        } else {
            let child = dir.join("style-tool.json");
            let mut raw: Value = serde_json::from_slice(&original_child).expect("JSON");
            if mutation == "child-tool" {
                raw["tool"]["name"] = serde_json::json!("cpp-infra-coverage");
            } else {
                raw["tool"]["revision"] = serde_json::json!("b".repeat(40));
            }
            let bytes = serde_json::to_vec(&raw).expect("JSON");
            fs::write(&child, &bytes).expect("raw mutation");
            use sha2::Digest;
            use sha2::Sha256;
            value["artifacts"][0]["sha256"] = serde_json::json!(
                Sha256::digest(&bytes)
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect::<String>()
            );
        }
        fs::write(&file, serde_json::to_vec(&value).expect("JSON")).expect("mutate wrapper");
        assert_eq!(
            invoke(&root, &["summarize", "--input-dir", dir.to_str().expect("dir")])
                .status
                .code(),
            Some(2),
            "must reject {mutation}"
        );
    }
}
#[cfg(unix)]
#[test]
fn test_review_execute_preserves_unmanaged_run_manifest() {
    for canonical in [false, true] {
        let root = fixture("'style'", "");
        let bin = binaries(&root);
        let business = root.path().join("business/run-manifest.json");
        if !canonical {
            fs::create_dir_all(business.parent().expect("parent")).expect("business");
            fs::write(&business, "{\"important\":\"data\"}").expect("business");
        }
        let manifest = root.path().join("build/plan.json");
        assert!(
            invoke(&root, &["plan", "--output", manifest.to_str().expect("path")])
                .status
                .success()
        );
        let plan: Value = serde_json::from_slice(&fs::read(&manifest).expect("manifest")).expect("JSON");
        let output = if canonical {
            root.path()
                .join("build/infra-runs")
                .join(plan["run_id"].as_str().expect("run"))
        } else {
            root.path().join("business")
        };
        fs::create_dir_all(&output).expect("output");
        let business = output.join("run-manifest.json");
        fs::write(&business, "{\"important\":\"data\"}").expect("business file");
        let out = run_with(
            &root,
            &bin,
            &[
                "execute",
                "--manifest",
                manifest.to_str().expect("manifest"),
                "--task-ids",
                "doctor-style,style",
                "--output-dir",
                output.to_str().expect("output"),
            ],
            &[],
        );
        assert_eq!(out.status.code(), Some(2));
        assert_eq!(
            fs::read_to_string(business).expect("preserved"),
            "{\"important\":\"data\"}"
        );
    }
}
#[cfg(unix)]
#[test]
fn test_review_export_rejects_destination_symlink_before_writing() {
    use sha2::Digest;
    use sha2::Sha256;
    let root = fixture("'style'", "");
    let bin = binaries(&root);
    let out = run_with(&root, &bin, &["check"], &[("CI_FAKE_ARTIFACT", "style")]);
    assert!(out.status.success());
    let summary: Value = serde_json::from_slice(&out.stdout).expect("summary");
    let dir = std::path::Path::new(summary["report_root"].as_str().expect("root"));
    fs::create_dir(dir.join("nested")).expect("nested");
    fs::write(dir.join("nested/payload.txt"), "payload").expect("payload");
    let mut wrapper: Value = serde_json::from_slice(&fs::read(dir.join("style.json")).expect("wrapper")).expect("JSON");
    let mut raw: Value = serde_json::from_slice(&fs::read(dir.join("style-tool.json")).expect("raw")).expect("JSON");
    raw["artifacts"] = serde_json::json!([{"kind":"result","path":"nested/payload.txt","sha256":Sha256::digest(b"payload").iter().map(|byte| format!("{byte:02x}")).collect::<String>(),"required":true}]);
    let bytes = serde_json::to_vec(&raw).expect("raw JSON");
    fs::write(dir.join("style-tool.json"), &bytes).expect("raw");
    wrapper["artifacts"][0]["sha256"] = serde_json::json!(
        Sha256::digest(bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    );
    fs::write(
        dir.join("style.json"),
        serde_json::to_vec(&wrapper).expect("wrapper JSON"),
    )
    .expect("wrapper");
    let outside = tempfile::tempdir().expect("outside");
    let output = root.path().join("build/export-review");
    fs::create_dir(&output).expect("export");
    std::os::unix::fs::symlink(outside.path(), output.join("nested")).expect("symlink");
    let out = invoke(
        &root,
        &[
            "export",
            "--input-dir",
            dir.to_str().expect("input"),
            "--task-ids",
            "style",
            "--output-dir",
            output.to_str().expect("output"),
        ],
    );
    assert_eq!(out.status.code(), Some(2));
    assert!(!outside.path().join("payload.txt").exists());
    assert!(
        !output.join("style.json").exists(),
        "all destination checks must precede writes"
    );
}
#[cfg(unix)]
#[test]
fn test_review_pages_site_is_discovered_from_private_artifact() {
    let root = fixture("'style','pages'", "");
    let bin = binaries(&root);
    let out = run_with(&root, &bin, &["check"], &[("CI_FAKE_SITE", "pages")]);
    assert!(out.status.success());
    let summary: Value = serde_json::from_slice(&out.stdout).expect("summary");
    let dir = std::path::Path::new(summary["report_root"].as_str().expect("root"));
    let out = invoke(
        &root,
        &[
            "artifact-path",
            "--manifest",
            dir.join("run-manifest.json").to_str().expect("manifest"),
            "--input-dir",
            dir.to_str().expect("input"),
            "--task-id",
            "pages",
            "--kind",
            "site",
        ],
    );
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let artifact: Value = serde_json::from_slice(&out.stdout).expect("artifact JSON");
    assert_eq!(artifact["path"], serde_json::json!(dir.join("pages/call-fixture/site")));
}
#[test]
fn test_review_matrix_doctors_execute_on_own_runner_and_profile() {
    let root = fixture(
        "'verify','analyze','coverage'",
        "[[matrix]]\nid='linux'\nrunner='ubuntu-latest'\nos='linux'\narch='x86_64'\nprofile='debug'\nsuites=['build','test']\n[[matrix]]\nid='windows'\nrunner='windows-latest'\nos='windows'\narch='x86_64'\nprofile='analysis'\nsuites=['build','test']",
    );
    let out = invoke(&root, &["matrix", "plan"]);
    assert!(out.status.success());
    let value: Value = serde_json::from_slice(&out.stdout).expect("matrix");
    for job in value["include"].as_array().expect("jobs") {
        let ids = job["task_ids"].as_str().expect("ids");
        let doctors: Vec<_> = value["plan"]["nodes"]
            .as_array()
            .expect("nodes")
            .iter()
            .filter(|node| ids.split(',').any(|id| node["id"] == id) && node["tool"] == "cpp-infra-tools")
            .collect();
        assert!(!doctors.is_empty(), "runner {} needs its own doctor", job["runner"]);
        assert!(doctors.iter().any(|doctor| doctor["profile"] == job["profile"]));
    }
    let analysis = value["analysis_task_ids"].as_str().expect("analysis");
    assert!(analysis.split(',').any(|id| id == "doctor-analyze"));
    assert!(analysis.split(',').any(|id| id == "doctor-coverage"));
}
#[cfg(unix)]
#[test]
fn test_review_config_inputs_cannot_reside_in_digest_blind_area() {
    for source in [".infra/project.toml", ".infra/ci/config.toml"] {
        let root = fixture("'style'", "");
        fs::create_dir(root.path().join("build")).expect("build");
        let destination = root.path().join("build/hidden.toml");
        fs::copy(root.path().join(source), &destination).expect("hidden config");
        fs::remove_file(root.path().join(source)).expect("replace disposable input");
        std::os::unix::fs::symlink(destination, root.path().join(source)).expect("input symlink");
        assert_eq!(
            invoke(&root, &["plan"]).status.code(),
            Some(2),
            "config {source} must stay inside hashed inputs"
        );
    }
}

#[test]
fn test_review_matrix_scope_names_do_not_alias_profile_names() {
    let root = fixture(
        "'verify'",
        "[[matrix]]\nid='linux'\nrunner='ubuntu-latest'\nos='linux'\narch='x86_64'\nprofile='debug'\nsuites=['build','test']",
    );
    let path = root.path().join(".infra/project.toml");
    let mut project = fs::read_to_string(&path).expect("project");
    project.push_str("\n[profiles.matrix-linux-debug]\nconfigure='other'\nbuild='other'\n");
    fs::write(path, project).expect("profile");
    let path = root.path().join(".infra/ci/config.toml");
    let config = fs::read_to_string(&path)
        .expect("config")
        .replace("primary_profile='debug'", "primary_profile='matrix-linux-debug'");
    fs::write(path, config).expect("config");
    let out = invoke(&root, &["matrix", "plan"]);
    assert!(out.status.success());
    let value: Value = serde_json::from_slice(&out.stdout).expect("plan");
    let nodes = value["plan"]["nodes"].as_array().expect("nodes");
    assert_eq!(
        nodes
            .iter()
            .filter(|node| node["profile"] == "matrix-linux-debug" && node["args"][0] == "run")
            .count(),
        2
    );
}

#[cfg(unix)]
#[test]
fn test_review_managed_run_cannot_follow_a_log_symlink() {
    let root = fixture("'style'", "");
    let bin = binaries(&root);
    let out = run_with(&root, &bin, &["check"], &[]);
    assert!(out.status.success());
    let value: Value = serde_json::from_slice(&out.stdout).expect("summary");
    let directory = std::path::Path::new(value["report_root"].as_str().expect("root"));
    let outside = tempfile::tempdir().expect("outside");
    let business = outside.path().join("important.txt");
    fs::write(&business, "keep this data").expect("business");
    fs::remove_file(directory.join("style.log")).expect("replace disposable log");
    std::os::unix::fs::symlink(&business, directory.join("style.log")).expect("log symlink");
    let out = run_with(
        &root,
        &bin,
        &[
            "execute",
            "--manifest",
            directory.join("run-manifest.json").to_str().expect("manifest"),
            "--task-ids",
            "style",
            "--output-dir",
            directory.to_str().expect("root"),
        ],
        &[],
    );
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(fs::read_to_string(business).expect("preserved"), "keep this data");
}

#[cfg(unix)]
#[test]
fn test_review_isolated_jobs_export_import_quality_pages_and_final() {
    fn copy_tree(source: &std::path::Path, target: &std::path::Path) {
        fs::create_dir_all(target).expect("target");
        for entry in fs::read_dir(source).expect("source") {
            let entry = entry.expect("entry");
            if entry.file_type().expect("kind").is_dir() {
                copy_tree(&entry.path(), &target.join(entry.file_name()));
            } else {
                let destination = target.join(entry.file_name());
                assert!(!destination.exists(), "job exports must not collide");
                fs::copy(entry.path(), destination).expect("copy evidence");
            }
        }
    }
    let matrix = "[[matrix]]\nid='linux-one'\nrunner='ubuntu-latest'\nos='linux'\narch='x86_64'\nprofile='debug'\nsuites=['build','test']\n[[matrix]]\nid='linux-two'\nrunner='self-hosted-linux'\nos='linux'\narch='x86_64'\nprofile='debug'\nsuites=['build','test']";
    let first = fixture("'verify','pages'", matrix);
    let second = fixture("'verify','pages'", matrix);
    let aggregate = fixture("'verify','pages'", matrix);
    let one_bin = binaries(&first);
    let two_bin = binaries(&second);
    let summary_bin = binaries(&aggregate);
    let out = invoke(&first, &["matrix", "plan"]);
    assert!(out.status.success());
    let matrix: Value = serde_json::from_slice(&out.stdout).expect("matrix");
    let plan = &matrix["plan"];
    let id = plan["run_id"].as_str().expect("run");
    let aggregate_output = aggregate.path().join("build/infra-runs").join(id);
    fs::create_dir_all(&aggregate_output).expect("aggregate root");
    let aggregate_manifest = aggregate.path().join("build/plan.json");
    fs::write(&aggregate_manifest, serde_json::to_vec(plan).expect("manifest")).expect("manifest");
    for (index, (project, bin)) in [(&first, &one_bin), (&second, &two_bin)].into_iter().enumerate() {
        let manifest = project.path().join("build/plan.json");
        fs::write(&manifest, serde_json::to_vec(plan).expect("manifest")).expect("manifest");
        let directory = project.path().join("build/infra-runs").join(id);
        let ids = matrix["include"][index]["task_ids"].as_str().expect("job ids");
        let out = run_with(
            project,
            bin,
            &[
                "execute",
                "--manifest",
                manifest.to_str().expect("manifest"),
                "--task-ids",
                ids,
                "--output-dir",
                directory.to_str().expect("root"),
                "--require-existing-dependencies",
            ],
            &[],
        );
        assert!(
            out.status.success(),
            "job {index}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let exported = project.path().join("build/owned");
        assert!(
            invoke(
                project,
                &[
                    "export",
                    "--input-dir",
                    directory.to_str().expect("root"),
                    "--task-ids",
                    ids,
                    "--output-dir",
                    exported.to_str().expect("export")
                ]
            )
            .status
            .success()
        );
        copy_tree(&exported, &aggregate_output);
    }
    for ids in [
        "quality-summary",
        matrix["pages_task_ids"].as_str().expect("pages"),
        "final-summary",
    ] {
        let out = run_with(
            &aggregate,
            &summary_bin,
            &[
                "execute",
                "--manifest",
                aggregate_manifest.to_str().expect("manifest"),
                "--task-ids",
                ids,
                "--output-dir",
                aggregate_output.to_str().expect("root"),
                "--require-existing-dependencies",
            ],
            &[("CI_FAKE_SITE", "pages")],
        );
        assert!(out.status.success(), "{ids}: {}", String::from_utf8_lossy(&out.stderr));
    }
    let final_report: Value =
        serde_json::from_slice(&fs::read(aggregate_output.join("final-summary.json")).expect("final")).expect("JSON");
    assert_eq!(final_report["status"], "passed");
    assert_eq!(final_report["data"]["complete"], true);
    let path = invoke(
        &aggregate,
        &[
            "artifact-path",
            "--manifest",
            aggregate_manifest.to_str().expect("manifest"),
            "--input-dir",
            aggregate_output.to_str().expect("root"),
            "--task-id",
            "pages",
            "--kind",
            "site",
        ],
    );
    assert!(path.status.success(), "{}", String::from_utf8_lossy(&path.stderr));
}

#[cfg(unix)]
#[test]
fn test_review_source_exclusion_keeps_raw_canonical_spelling() {
    let root = fixture("'style'", "");
    let bin = binaries(&root);
    let out = invoke(&root, &["plan"]);
    let plan: Value = serde_json::from_slice(&out.stdout).expect("plan");
    fs::write(root.path().join("business.json"), "{\"important\":true}").expect("new business JSON");
    let file = root.path().join("build/context.json");
    for raw in [
        "business.json/",
        "./business.json",
        "business.json//",
        ".//business.json",
        "nested/../business.json",
        "nested\\business.json",
    ] {
        let context = serde_json::json!({"schema":1,"run_id":plan["run_id"],"task_id":"ci","bundle_id":plan["bundle_id"],"project":root.path().canonicalize().expect("root"),"profile":"debug","source_digest":plan["source_digest"],"source_excluded_paths":[raw],"config_digest":plan["config_digest"],"report_root":root.path().canonicalize().expect("root").join("build/infra-runs").join(plan["run_id"].as_str().expect("run"))});
        fs::write(&file, serde_json::to_vec(&context).expect("JSON")).expect("context");
        let out = run_with(
            &root,
            &bin,
            &["--run-context", file.to_str().expect("context"), "check"],
            &[],
        );
        assert_eq!(out.status.code(), Some(2));
        assert!(
            String::from_utf8_lossy(&out.stderr).contains("invalid source exclusion"),
            "raw {raw} must fail before hashing: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    assert!(!root.path().join("build/calls.log").exists());
    assert_eq!(
        fs::read_to_string(root.path().join("business.json")).expect("business JSON"),
        "{\"important\":true}"
    );
}

#[test]
fn test_review_package_reuses_the_primary_profiles_matrix_runner() {
    let root = fixture(
        "'verify','package'",
        "[[matrix]]\nid='windows'\nrunner='windows-latest'\nos='windows'\narch='x86_64'\nprofile='debug'\nsuites=['build','test']",
    );
    let out = invoke(&root, &["matrix", "plan"]);
    assert!(out.status.success());
    let value: Value = serde_json::from_slice(&out.stdout).expect("matrix");
    let jobs = value["include"].as_array().expect("jobs");
    assert_eq!(
        jobs.len(),
        1,
        "package must not create an Ubuntu job for a Windows primary profile"
    );
    assert_eq!(jobs[0]["runner"], "windows-latest");
    let ids = jobs[0]["task_ids"].as_str().expect("ids");
    assert!(
        value["plan"]["nodes"]
            .as_array()
            .expect("nodes")
            .iter()
            .any(|node| ids.split(',').any(|id| node["id"] == id)
                && node["args"] == serde_json::json!(["run", "--profile", "debug", "--suite", "package"]))
    );
}

#[cfg(unix)]
#[test]
fn test_rereview_tracked_config_retarget_cannot_reuse_plan() {
    let root = fixture("'style'", "");
    let bin = binaries(&root);
    let entry = root.path().join(".infra/ci/config.toml");
    let first = fs::read_to_string(&entry).expect("CI config");
    fs::write(root.path().join(".infra/a.toml"), &first).expect("first config");
    fs::write(
        root.path().join(".infra/b.toml"),
        first.replace("tasks=['style']", "tasks=['verify']"),
    )
    .expect("second config");
    fs::remove_file(&entry).expect("replace disposable config");
    std::os::unix::fs::symlink("../a.toml", &entry).expect("config selector");
    let manifest = root.path().join("build/linked-plan.json");
    let out = invoke(&root, &["plan", "--output", manifest.to_str().expect("manifest")]);
    if !out.status.success() {
        assert_eq!(out.status.code(), Some(2));
        assert!(String::from_utf8_lossy(&out.stderr).contains("symlink"));
        assert!(!manifest.exists());
        return;
    }
    let plan: Value = serde_json::from_slice(&out.stdout).expect("legacy linked plan");
    fs::remove_file(&entry).expect("retarget disposable selector");
    std::os::unix::fs::symlink("../b.toml", &entry).expect("new selector");
    let output = root
        .path()
        .canonicalize()
        .expect("root")
        .join("build/infra-runs")
        .join(plan["run_id"].as_str().expect("run"));
    let out = run_with(
        &root,
        &bin,
        &[
            "execute",
            "--manifest",
            manifest.to_str().expect("manifest"),
            "--task-ids",
            "doctor-style,style,quality-summary,final-summary",
            "--output-dir",
            output.to_str().expect("output"),
        ],
        &[],
    );
    assert_eq!(
        out.status.code(),
        Some(2),
        "retargeted config must not execute an old style-only plan"
    );
}
#[cfg(unix)]
#[test]
fn test_rereview_all_consumed_config_files_and_ancestors_reject_links() {
    for source in [
        ".infra/project.toml",
        ".infra/ci/config.toml",
        ".infra/style/config.toml",
    ] {
        let root = fixture("'style'", "");
        let entry = root.path().join(source);
        let target = root.path().join(".infra/tracked-input.toml");
        fs::copy(&entry, &target).expect("tracked target");
        fs::remove_file(&entry).expect("replace disposable input");
        std::os::unix::fs::symlink(&target, &entry).expect("tracked config link");
        let out = invoke(&root, &["plan"]);
        assert_eq!(out.status.code(), Some(2), "must reject config selector {source}");
        assert!(String::from_utf8_lossy(&out.stderr).contains("symlink"));
    }
    let root = fixture("'style'", "");
    let original = root.path().join(".infra/ci");
    let target = root.path().join(".infra/tracked-ci");
    fs::create_dir(&target).expect("tracked directory");
    fs::copy(original.join("config.toml"), target.join("config.toml")).expect("tracked config");
    fs::remove_file(original.join("config.toml")).expect("remove disposable fixture file");
    fs::remove_dir(&original).expect("replace disposable fixture directory");
    std::os::unix::fs::symlink(target, original).expect("config ancestor link");
    assert_eq!(invoke(&root, &["matrix", "plan"]).status.code(), Some(2));
}
#[cfg(unix)]
#[test]
fn test_rereview_source_exclusions_reject_drive_syntax_on_every_host() {
    let root = fixture("'style'", "");
    let bin = binaries(&root);
    let out = invoke(&root, &["plan"]);
    let plan: Value = serde_json::from_slice(&out.stdout).expect("plan");
    let context_path = root.path().join("build/drive-context.json");
    for path in ["C:report.json", "c:/report.json", "Z:dir/report.json"] {
        let context = serde_json::json!({"schema":1,"run_id":plan["run_id"],"task_id":"ci","bundle_id":plan["bundle_id"],"project":root.path().canonicalize().expect("root"),"profile":"debug","source_digest":plan["source_digest"],"source_excluded_paths":[path],"config_digest":plan["config_digest"],"report_root":root.path().canonicalize().expect("root").join("build/infra-runs").join(plan["run_id"].as_str().expect("run"))});
        fs::write(&context_path, serde_json::to_vec(&context).expect("JSON")).expect("context");
        let out = run_with(
            &root,
            &bin,
            &["--run-context", context_path.to_str().expect("context"), "check"],
            &[],
        );
        assert_eq!(
            out.status.code(),
            Some(2),
            "drive syntax {path} cannot be a portable relative exclusion"
        );
        assert!(String::from_utf8_lossy(&out.stderr).contains("invalid source exclusion"));
    }
    let destination = root.path().join("C:explicit.json");
    let out = invoke(&root, &["--report", destination.to_str().expect("report"), "plan"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(!destination.exists());
}
#[cfg(unix)]
#[test]
fn test_rereview_summary_accepts_external_absolute_manifest_with_provenance() {
    use sha2::Digest;
    use sha2::Sha256;
    let root = fixture("'style'", "");
    let bin = binaries(&root);
    let out = run_with(&root, &bin, &["check"], &[]);
    assert!(out.status.success());
    let summary: Value = serde_json::from_slice(&out.stdout).expect("summary");
    let input = std::path::Path::new(summary["report_root"].as_str().expect("root"));
    let bytes = fs::read(input.join("run-manifest.json")).expect("manifest");
    let outside = tempfile::tempdir().expect("external location");
    let external = outside.path().join("manifest.json");
    fs::write(&external, &bytes).expect("external manifest");
    let out = invoke(
        &root,
        &[
            "summarize",
            "--input-dir",
            input.to_str().expect("input"),
            "--manifest",
            external.to_str().expect("manifest"),
        ],
    );
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let value: Value = serde_json::from_slice(&out.stdout).expect("summary");
    assert_eq!(value["status"], "passed");
    assert_eq!(
        value["data"]["manifest_provenance"]["sha256"],
        Sha256::digest(&bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    );
    assert_eq!(
        value["data"]["manifest_provenance"]["path"],
        serde_json::json!(external.canonicalize().expect("canonical manifest"))
    );
    assert!(value["artifacts"].as_array().expect("artifacts").is_empty());
    assert_eq!(fs::read(&external).expect("untouched manifest"), bytes);
}
#[cfg(unix)]
#[test]
fn test_rereview_summary_accepts_project_relative_manifest() {
    let root = fixture("'style'", "");
    let bin = binaries(&root);
    let out = run_with(&root, &bin, &["check"], &[("CI_FAKE_ARTIFACT", "style")]);
    assert!(out.status.success());
    let summary: Value = serde_json::from_slice(&out.stdout).expect("summary");
    let input = std::path::Path::new(summary["report_root"].as_str().expect("root"));
    fs::copy(
        input.join("run-manifest.json"),
        root.path().join("build/external-manifest.json"),
    )
    .expect("external copy");
    let out = invoke(
        &root,
        &[
            "summarize",
            "--input-dir",
            input.to_str().expect("input"),
            "--manifest",
            "build/external-manifest.json",
        ],
    );
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(
        serde_json::from_slice::<Value>(&out.stdout).expect("summary")["status"],
        "passed"
    );

    let out = run_with(
        &root,
        &bin,
        &[
            "execute",
            "--manifest",
            "build/external-manifest.json",
            "--task-ids",
            "final-summary",
            "--output-dir",
            input.to_str().expect("root"),
            "--require-existing-dependencies",
        ],
        &[],
    );
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let out = invoke(
        &root,
        &[
            "artifact-path",
            "--manifest",
            "build/external-manifest.json",
            "--input-dir",
            input.to_str().expect("root"),
            "--task-id",
            "style",
            "--kind",
            "result",
        ],
    );
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(
        serde_json::from_slice::<Value>(&out.stdout).expect("artifact")["path"],
        serde_json::json!(input.join("style-data.txt"))
    );
}

#[cfg(unix)]
#[test]
fn test_runtime_snapshot_binds_copy_forwards_context_and_rejects_tampering() {
    use sha2::Digest;
    use sha2::Sha256;
    let root = fixture("'style'", "");
    let bin = binaries(&root);
    let version: Value = serde_json::from_slice(&invoke(&root, &["--version", "--json"]).stdout).expect("version");
    let plan: Value = serde_json::from_slice(&invoke(&root, &["plan"]).stdout).expect("plan");
    let project = root.path().canonicalize().expect("project");
    let output = project
        .join("build/infra-runs")
        .join(plan["run_id"].as_str().expect("run"));
    fs::create_dir_all(&output).expect("run root");
    let copy = output.join("ci-copy");
    fs::copy(env!("CARGO_BIN_EXE_cpp-infra-ci"), &copy).expect("same executable copy");
    let binary_sha = Sha256::digest(fs::read(&copy).expect("binary"))
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let snapshot_path = output.join("runtime-snapshot.json");
    let snapshot = serde_json::json!({"schema":1,"bundle_id":"development","tools":[{"name":"cpp-infra-ci","version":version["version"],"revision":version["revision"],"protocol":1,"binary":copy,"executable_sha256":binary_sha}],"offline":true,"stale":false});
    let snapshot_bytes = serde_json::to_vec(&snapshot).expect("snapshot");
    fs::write(&snapshot_path, &snapshot_bytes).expect("snapshot file");
    let context_path = project.join("build/runtime-context.json");
    let mut context = serde_json::json!({"schema":1,"run_id":plan["run_id"],"task_id":"parent-ci","bundle_id":"development","project":project,"profile":"debug","source_digest":plan["source_digest"],"config_digest":plan["config_digest"],"report_root":output,"runtime_snapshot":snapshot_path,"runtime_snapshot_digest":Sha256::digest(&snapshot_bytes).iter().map(|byte| format!("{byte:02x}")).collect::<String>()});
    fs::write(&context_path, serde_json::to_vec(&context).expect("context")).expect("context file");
    let report = output.join("parent-ci.json");
    let out = run_with(
        &root,
        &bin,
        &[
            "--run-context",
            context_path.to_str().expect("context"),
            "--report",
            report.to_str().expect("report"),
            "check",
        ],
        &[],
    );
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let public: Value = serde_json::from_slice(&fs::read(&report).expect("public report")).expect("JSON");
    assert_eq!(public["task_id"], "parent-ci");
    assert_eq!(public["run_id"], plan["run_id"]);
    let child: Value =
        serde_json::from_slice(&fs::read(output.join("style-context.json")).expect("child context")).expect("JSON");
    assert_eq!(child["runtime_snapshot"], context["runtime_snapshot"]);
    assert_eq!(child["runtime_snapshot_digest"], context["runtime_snapshot_digest"]);
    let env_plan = run_with(
        &root,
        &bin,
        &["plan"],
        &[("CPP_INFRA_RUNTIME_SNAPSHOT", snapshot_path.to_str().expect("snapshot"))],
    );
    assert!(env_plan.status.success());
    for failure in [
        "digest",
        "schema",
        "bundle",
        "missing-tool",
        "version",
        "protocol",
        "sha",
        "copy",
    ] {
        let mut value = snapshot.clone();
        match failure {
            "schema" => value["schema"] = serde_json::json!(2),
            "bundle" => value["bundle_id"] = serde_json::json!("other-bundle"),
            "missing-tool" => value["tools"] = serde_json::json!([]),
            "version" => value["tools"][0]["version"] = serde_json::json!("999"),
            "protocol" => value["tools"][0]["protocol"] = serde_json::json!(2),
            "sha" => value["tools"][0]["executable_sha256"] = serde_json::json!("0".repeat(64)),
            "copy" => {
                fs::write(&copy, "changed copy").expect("tamper copied fixture");
            }
            _ => {}
        }
        let bytes = serde_json::to_vec(&value).expect("snapshot");
        fs::write(&snapshot_path, &bytes).expect("snapshot file");
        context["runtime_snapshot_digest"] = serde_json::json!(if failure == "digest" {
            "0".repeat(64)
        } else {
            Sha256::digest(&bytes)
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        });
        fs::write(&context_path, serde_json::to_vec(&context).expect("context")).expect("context file");
        let out = run_with(
            &root,
            &bin,
            &["--run-context", context_path.to_str().expect("context"), "plan"],
            &[],
        );
        assert_eq!(
            out.status.code(),
            Some(2),
            "must reject runtime {failure}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}
#[cfg(unix)]
#[test]
fn test_runtime_plan_pins_selected_tools_and_rejects_missing_or_invalid_revision() {
    use sha2::Digest;
    use sha2::Sha256;
    let root = fixture("'style'", "");
    let bin = binaries(&root);
    let version: Value = serde_json::from_slice(&invoke(&root, &["--version", "--json"]).stdout).expect("version");
    let binary = std::path::Path::new(env!("CARGO_BIN_EXE_cpp-infra-ci"))
        .canonicalize()
        .expect("binary");
    let mut snapshot = serde_json::json!({"schema":1,"bundle_id":"fixture-source-group","tools":[{"name":"cpp-infra-ci","version":version["version"],"revision":version["revision"],"protocol":1,"binary":binary,"executable_sha256":Sha256::digest(fs::read(&binary).expect("binary")).iter().map(|byte| format!("{byte:02x}")).collect::<String>()},{"name":"cpp-infra-tools","version":"0.1.0","revision":"a".repeat(40)},{"name":"cpp-infra-style","version":"0.1.0","revision":"b".repeat(40)}],"offline":true,"stale":false});
    for tool in snapshot["tools"].as_array_mut().expect("tools").iter_mut().skip(1) {
        let binary = bin.path().join(tool["name"].as_str().expect("tool name"));
        tool["protocol"] = serde_json::json!(1);
        tool["executable_sha256"] = serde_json::json!(
            Sha256::digest(fs::read(&binary).expect("fixture binary"))
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        );
        tool["binary"] = serde_json::json!(binary);
    }
    let path = root.path().join("build/pinned-runtime.json");
    fs::write(&path, serde_json::to_vec(&snapshot).expect("snapshot")).expect("runtime");
    let out = run_with(
        &root,
        &bin,
        &["plan"],
        &[("CPP_INFRA_RUNTIME_SNAPSHOT", path.to_str().expect("runtime"))],
    );
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let value: Value = serde_json::from_slice(&out.stdout).expect("plan");
    assert_eq!(value["bundle_id"], "fixture-source-group");
    assert!(
        value["nodes"]
            .as_array()
            .expect("nodes")
            .iter()
            .any(|node| node["tool"] == "cpp-infra-style"
                && node["revision"] == "b".repeat(40)
                && node["version"] == "0.1.0")
    );
    snapshot["tools"][2]["revision"] = serde_json::json!("bad");
    fs::write(&path, serde_json::to_vec(&snapshot).expect("snapshot")).expect("runtime");
    assert_eq!(
        run_with(
            &root,
            &bin,
            &["plan"],
            &[("CPP_INFRA_RUNTIME_SNAPSHOT", path.to_str().expect("runtime"))]
        )
        .status
        .code(),
        Some(2)
    );
    snapshot["tools"].as_array_mut().expect("tools").pop();
    fs::write(&path, serde_json::to_vec(&snapshot).expect("snapshot")).expect("runtime");
    let out = run_with(
        &root,
        &bin,
        &["plan"],
        &[("CPP_INFRA_RUNTIME_SNAPSHOT", path.to_str().expect("runtime"))],
    );
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("snapshot lacks cpp-infra-style"));
}
#[test]
fn test_plan_reads_detached_loose_packed_and_worktree_git_metadata_without_git() {
    let root = fixture("'style'", "");
    let git = root.path().join(".git");
    fs::create_dir_all(git.join("refs/heads")).expect("git metadata");
    let first = "a".repeat(40);
    let second = "b".repeat(40);
    let check = |expected: &str| {
        let out = invoke(&root, &["plan"]);
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        assert_eq!(
            serde_json::from_slice::<Value>(&out.stdout).expect("plan")["source_revision"],
            expected
        );
    };
    fs::write(git.join("HEAD"), &first).expect("detached head");
    check(&first);
    fs::write(git.join("HEAD"), "ref: refs/heads/main\n").expect("symbolic head");
    fs::write(git.join("refs/heads/main"), &second).expect("loose ref");
    check(&second);
    fs::remove_file(git.join("refs/heads/main")).expect("remove disposable loose fixture");
    fs::write(
        git.join("packed-refs"),
        format!("# packed refs\n{first} refs/heads/main\n"),
    )
    .expect("packed ref");
    check(&first);
    let worktree = fixture("'style'", "");
    let metadata = worktree.path().join("build/git-worktree");
    let common = worktree.path().join("build/git-common");
    fs::create_dir_all(&metadata).expect("metadata");
    fs::create_dir_all(common.join("refs/heads")).expect("common git dir");
    fs::write(worktree.path().join(".git"), "gitdir: build/git-worktree\n").expect("git pointer");
    fs::write(metadata.join("commondir"), "../git-common\n").expect("common pointer");
    fs::write(metadata.join("HEAD"), "ref: refs/heads/main\n").expect("head");
    fs::write(common.join("refs/heads/main"), &second).expect("worktree ref");
    let out = invoke(&worktree, &["plan"]);
    assert!(out.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&out.stdout).expect("plan")["source_revision"],
        second
    );
    fs::write(metadata.join("HEAD"), "not a git head").expect("invalid head");
    let out = invoke(&worktree, &["plan"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("invalid Git HEAD"));
}
#[cfg(unix)]
#[test]
fn test_dependency_and_standalone_package_execute_their_backend_contracts() {
    let root = fixture("'dependency','verify'", "");
    let path = root.path().join(".infra/project.toml");
    let project = fs::read_to_string(&path)
        .expect("project")
        .replace("test='debug'", "test='debug'\nconan_context='linux-debug'");
    fs::write(&path, project).expect("Conan binding");
    let bin = binaries(&root);
    let out = run_with(&root, &bin, &["check"], &[]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let calls = fs::read_to_string(root.path().join("build/calls.log")).expect("calls");
    assert!(calls.contains("check --context linux-debug"));
    let package = fixture("'package'", "");
    let bin = binaries(&package);
    let out = run_with(&package, &bin, &["check", "--only", "package"], &[]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let calls = fs::read_to_string(package.path().join("build/calls.log")).expect("calls");
    assert!(calls.contains("--task verify --profile debug"));
    assert!(calls.contains("--suite package"));
}
#[cfg(unix)]
#[test]
fn test_artifact_resolver_preserves_a_failed_pages_gate() {
    let root = fixture("'style','pages'", "");
    let bin = binaries(&root);
    let out = run_with(&root, &bin, &["check"], &[("CI_FAKE_FAIL", "pages")]);
    assert_eq!(out.status.code(), Some(1));
    let summary: Value = serde_json::from_slice(&out.stdout).expect("summary");
    let input = std::path::Path::new(summary["report_root"].as_str().expect("root"));
    let out = invoke(
        &root,
        &[
            "artifact-path",
            "--manifest",
            input.join("run-manifest.json").to_str().expect("manifest"),
            "--input-dir",
            input.to_str().expect("root"),
            "--task-id",
            "pages",
            "--kind",
            "site",
        ],
    );
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("artifact task did not pass"));
}
#[cfg(unix)]
#[test]
fn test_matrix_run_executes_only_the_selected_host_branch_and_rejects_remote() {
    let matrix = format!(
        "[[matrix]]\nid='host'\nrunner='local-host'\nos='{}'\narch='{}'\nprofile='debug'\nsuites=['test']\n[[matrix]]\nid='remote'\nrunner='windows-latest'\nos='windows'\narch='x86_64'\nprofile='analysis'\nsuites=['test']",
        std::env::consts::OS,
        std::env::consts::ARCH
    );
    let root = fixture("'verify'", &matrix);
    let bin = binaries(&root);
    let out = run_with(&root, &bin, &["matrix", "run", "--check", "host"], &[]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let calls = fs::read_to_string(root.path().join("build/calls.log")).expect("calls");
    assert!(calls.contains("job.matrix-host.verify-test"));
    assert!(!calls.contains("matrix-remote"));
    let out = run_with(&root, &bin, &["matrix", "run", "--check", "remote"], &[]);
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("does not match host"));
}

#[cfg(unix)]
#[test]
fn test_rereview_project_root_and_hook_selectors_cannot_hide_link_changes() {
    for hook in [false, true] {
        let root = fixture("'style'", "");
        if hook {
            fs::write(root.path().join("first-hook"), "#!/bin/sh\nexit 0\n").expect("first hook");
            fs::write(root.path().join("second-hook"), "#!/bin/sh\nexit 1\n").expect("second hook");
            std::os::unix::fs::symlink("first-hook", root.path().join("selected-hook")).expect("hook selector");
            let path = root.path().join(".infra/ci/config.toml");
            let config = fs::read_to_string(&path).expect("CI config")
                + "\n[[hooks]]\nid='custom'\nexecutable='selected-hook'\n";
            fs::write(path, config).expect("hook config");
        } else {
            for directory in ["first-src", "second-src"] {
                fs::create_dir(root.path().join(directory)).expect("source directory");
                fs::write(root.path().join(directory).join("source.cpp"), directory).expect("tracked source");
            }
            std::os::unix::fs::symlink("first-src", root.path().join("selected-src")).expect("source root selector");
            let path = root.path().join(".infra/project.toml");
            let project = fs::read_to_string(&path)
                .expect("project")
                .replace("source_roots=[]", "source_roots=['selected-src']");
            fs::write(path, project).expect("source roots");
        }
        let out = invoke(&root, &["plan"]);
        assert_eq!(
            out.status.code(),
            Some(2),
            "configured project selectors must have tracked ordinary paths"
        );
        assert!(String::from_utf8_lossy(&out.stderr).contains("symlink"));
    }
}

#[cfg(unix)]
#[test]
fn test_rereview_legacy_tool_lock_is_also_a_regular_configuration_input() {
    let root = fixture("'style'", "");
    let tools=["cpp-infra-tools","cpp-infra-style","cpp-infra-ci"].into_iter().map(|name|serde_json::json!({"name":name,"version":"0.1.0","revision":"a".repeat(40),"protocol":1,"assets":{}})).collect::<Vec<_>>();
    let snapshot = serde_json::json!({"schema":1,"protocol_major":1,"bundle_id":"fixture-lock","tools":tools});
    let target = root.path().join(".infra/tracked-lock.json");
    fs::write(&target, serde_json::to_vec(&snapshot).expect("snapshot")).expect("tracked lock");
    std::os::unix::fs::symlink(target, root.path().join(".infra/tools/tools.lock.json")).expect("lock selector");
    let out = invoke(&root, &["plan"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("symlink"));
}

#[cfg(unix)]
#[test]
fn test_offline_path_discovery_and_locked_binary_directory_do_not_mix() {
    let root = fixture("'style'", "");
    let preferred = binaries(&root);
    let fallback = binaries(&root);
    let inherited = std::env::var_os("PATH").expect("PATH");
    let paths = std::iter::once(fallback.path().to_path_buf()).chain(std::env::split_paths(&inherited));
    let path = std::env::join_paths(paths).expect("PATH entries");
    let invoke = |directory: Option<&std::path::Path>| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_cpp-infra-ci"));
        command
            .arg("--project")
            .arg(root.path())
            .args(["--offline", "check"])
            .env_remove("CPP_INFRA_BIN_DIR")
            .env("PATH", &path);
        if let Some(directory) = directory {
            command.env("CPP_INFRA_BIN_DIR", directory);
        }
        command.output().expect("CI invocation")
    };
    let out = invoke(None);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let before = fs::read_to_string(root.path().join("build/calls.log")).expect("calls");
    assert!(before.lines().all(|line| line.contains("--offline")));
    assert_eq!(before.lines().filter(|line| line.starts_with("style ")).count(), 1);
    fs::remove_file(preferred.path().join("cpp-infra-style")).expect("remove disposable binary");
    let out = invoke(Some(preferred.path()));
    assert_eq!(out.status.code(), Some(3));
    let after = fs::read_to_string(root.path().join("build/calls.log")).expect("calls");
    assert_eq!(
        after.lines().filter(|line| line.starts_with("style ")).count(),
        1,
        "an existing binary directory may not fall back to another bundle on PATH"
    );
    let summary: Value = serde_json::from_slice(&out.stdout).expect("summary");
    assert_eq!(summary["status"], "failed");
}

#[cfg(unix)]
fn run_with_runtime_parent(
    root: &TempDir,
    bin: &TempDir,
    plan: &Value,
    parent_id: &str,
    args: &[&str],
    extra: &[(&str, &str)],
) -> Output {
    use sha2::Digest;
    use sha2::Sha256;
    let project = root.path().canonicalize().expect("project");
    let parent = project.join("build/infra-runs").join(parent_id);
    fs::create_dir_all(&parent).expect("manager run root");
    let version: Value = serde_json::from_slice(&invoke(root, &["--version", "--json"]).stdout).expect("CI identity");
    let binary = std::path::Path::new(env!("CARGO_BIN_EXE_cpp-infra-ci"))
        .canonicalize()
        .expect("CI executable");
    let snapshot = serde_json::json!({"schema":1,"bundle_id":plan["bundle_id"],"tools":[{"name":"cpp-infra-ci","version":version["version"],"revision":version["revision"],"protocol":1,"binary":binary,"executable_sha256":Sha256::digest(fs::read(&binary).expect("CI bytes")).iter().map(|byte| format!("{byte:02x}")).collect::<String>()}],"offline":true,"stale":false});
    let snapshot_path = parent.join("runtime-snapshot.json");
    let snapshot_bytes = serde_json::to_vec_pretty(&snapshot).expect("snapshot");
    fs::write(&snapshot_path, &snapshot_bytes).expect("manager snapshot");
    let context = serde_json::json!({"schema":1,"run_id":parent_id,"task_id":"tools","project":project,"report_root":parent,"bundle_id":plan["bundle_id"],"profile":null,"source_digest":plan["source_digest"],"config_digest":plan["config_digest"],"source_excluded_paths":plan["source_excluded_paths"],"runtime_snapshot":snapshot_path,"runtime_snapshot_digest":Sha256::digest(&snapshot_bytes).iter().map(|byte| format!("{byte:02x}")).collect::<String>()});
    let context_path = parent.join("run-context.json");
    let context_bytes = serde_json::to_vec(&context).expect("context");
    fs::write(&context_path, &context_bytes).expect("manager context");
    let report = parent.join("ci-child.json");
    let mut arguments = vec![
        "--run-context",
        context_path.to_str().expect("context path"),
        "--report",
        report.to_str().expect("report path"),
    ];
    arguments.extend_from_slice(args);
    let mut environment = vec![
        (
            "CPP_INFRA_RUNTIME_SNAPSHOT",
            snapshot_path.to_str().expect("snapshot path"),
        ),
        ("CI_FAKE_STRICT_SNAPSHOT", "1"),
    ];
    environment.extend_from_slice(extra);
    let output = run_with(root, bin, &arguments, &environment);
    let top: Value = serde_json::from_slice(&fs::read(&report).expect("top report")).expect("public report");
    assert_eq!(top["run_id"], parent_id);
    assert_eq!(top["task_id"], "tools");
    assert_eq!(top["report_root"], serde_json::json!(parent));
    assert_eq!(
        fs::read(context_path).expect("parent context after invocation"),
        context_bytes,
        "top parent context must remain unchanged"
    );
    output
}
#[cfg(unix)]
#[test]
fn test_family_distributed_snapshot_stays_local_across_new_manager_invocations() {
    fn copy_evidence(source: &std::path::Path, destination: &std::path::Path) {
        fs::create_dir_all(destination).expect("destination");
        for entry in fs::read_dir(source).expect("exported evidence") {
            let entry = entry.expect("entry");
            assert!(
                !entry.file_name().to_string_lossy().contains("runtime"),
                "host-local snapshots must not be portable artifacts"
            );
            if entry.file_type().expect("file type").is_dir() {
                copy_evidence(&entry.path(), &destination.join(entry.file_name()));
            } else {
                fs::copy(entry.path(), destination.join(entry.file_name())).expect("import evidence");
            }
        }
    }
    let first = fixture("'style','pages'", "");
    let second = fixture("'style','pages'", "");
    let first_bin = binaries(&first);
    let second_bin = binaries(&second);
    let planned = invoke(&first, &["plan"]);
    assert!(planned.status.success());
    let plan: Value = serde_json::from_slice(&planned.stdout).expect("shared plan");
    let id = plan["run_id"].as_str().expect("shared run");
    let first_output = first.path().join("build/infra-runs").join(id);
    fs::create_dir_all(&first_output).expect("logical root");
    fs::write(first_output.join("runtime-snapshot.json"), "business file").expect("existing business output");
    let first_manifest = first.path().join("build/manifest.json");
    fs::write(&first_manifest, &planned.stdout).expect("manifest");
    let result = run_with_runtime_parent(
        &first,
        &first_bin,
        &plan,
        "manager-producer",
        &[
            "execute",
            "--manifest",
            first_manifest.to_str().expect("manifest"),
            "--task-ids",
            "doctor-style,style",
            "--output-dir",
            first_output.to_str().expect("logical root"),
            "--require-existing-dependencies",
        ],
        &[],
    );
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stdout));
    assert_eq!(
        fs::read_to_string(first_output.join("runtime-snapshot.json")).expect("business output"),
        "business file"
    );
    let original = fs::read(
        first
            .path()
            .join("build/infra-runs/manager-producer/runtime-snapshot.json"),
    )
    .expect("authenticated local snapshot");
    let mut used_path = None;
    for task in ["doctor-style", "style"] {
        let context: Value = serde_json::from_slice(
            &fs::read(first_output.join(format!("{task}-context.json"))).expect("child context"),
        )
        .expect("context");
        let path = std::path::Path::new(context["runtime_snapshot"].as_str().expect("snapshot reference"));
        assert!(path.starts_with(&first_output));
        assert_eq!(fs::read(path).expect("relocated snapshot"), original);
        let wrapper: Value =
            serde_json::from_slice(&fs::read(first_output.join(format!("{task}.json"))).expect("wrapper"))
                .expect("wrapper JSON");
        assert_eq!(
            wrapper["commands"][0]["env"]["CPP_INFRA_RUNTIME_SNAPSHOT"],
            context["runtime_snapshot"]
        );
        if let Some(previous) = &used_path {
            assert_eq!(path, previous);
        } else {
            used_path = Some(path.to_path_buf());
        }
    }
    let exported = first.path().join("build/exported");
    let result = run_with_runtime_parent(
        &first,
        &first_bin,
        &plan,
        "manager-export",
        &[
            "export",
            "--input-dir",
            first_output.to_str().expect("input"),
            "--task-ids",
            "doctor-style,style",
            "--output-dir",
            exported.to_str().expect("export"),
        ],
        &[],
    );
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    let second_output = second.path().join("build/infra-runs").join(id);
    copy_evidence(&exported, &second_output);
    let second_manifest = second.path().join("build/manifest.json");
    fs::write(&second_manifest, &planned.stdout).expect("consumer manifest");
    for (parent, tasks) in [
        ("manager-quality", "quality-summary"),
        ("manager-pages", "doctor-pages,pages"),
        ("manager-final", "final-summary"),
    ] {
        let result = run_with_runtime_parent(
            &second,
            &second_bin,
            &plan,
            parent,
            &[
                "execute",
                "--manifest",
                second_manifest.to_str().expect("manifest"),
                "--task-ids",
                tasks,
                "--output-dir",
                second_output.to_str().expect("logical root"),
                "--require-existing-dependencies",
            ],
            &[("CI_FAKE_SITE", "pages")],
        );
        assert!(
            result.status.success(),
            "{parent}: {} {}",
            String::from_utf8_lossy(&result.stderr),
            String::from_utf8_lossy(&result.stdout)
        );
    }
    let result = run_with_runtime_parent(
        &second,
        &second_bin,
        &plan,
        "manager-artifact",
        &[
            "artifact-path",
            "--manifest",
            second_manifest.to_str().expect("manifest"),
            "--input-dir",
            second_output.to_str().expect("root"),
            "--task-id",
            "pages",
            "--kind",
            "site",
        ],
        &[],
    );
    assert!(result.status.success());
    let result = run_with_runtime_parent(
        &second,
        &second_bin,
        &plan,
        "manager-summary",
        &[
            "summarize",
            "--manifest",
            second_manifest.to_str().expect("manifest"),
            "--input-dir",
            second_output.to_str().expect("root"),
        ],
        &[],
    );
    assert!(result.status.success());
    let final_report: Value =
        serde_json::from_slice(&fs::read(second_output.join("final-summary.json")).expect("final report"))
            .expect("JSON");
    assert_eq!(final_report["status"], "passed");
    assert_eq!(final_report["data"]["complete"], true);
    let pages: Value =
        serde_json::from_slice(&fs::read(second_output.join("pages-context.json")).expect("pages context"))
            .expect("JSON");
    let pages_snapshot = std::path::Path::new(pages["runtime_snapshot"].as_str().expect("snapshot"));
    assert!(pages_snapshot.starts_with(&second_output));
    assert_eq!(fs::read(pages_snapshot).expect("consumer snapshot"), original);
}
#[cfg(unix)]
#[test]
fn test_family_check_retains_parent_snapshot_and_pins_its_authenticated_digest() {
    let root = fixture("'style'", "");
    let bin = binaries(&root);
    let plan: Value = serde_json::from_slice(&invoke(&root, &["plan"]).stdout).expect("plan");
    let result = run_with_runtime_parent(&root, &bin, &plan, "manager-check", &["check"], &[]);
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stdout));
    let result = run_with_runtime_parent(
        &root,
        &bin,
        &plan,
        "manager-tampered",
        &["check"],
        &[("CI_FAKE_MUTATE_RUNTIME", "doctor-style")],
    );
    assert_eq!(
        result.status.code(),
        Some(2),
        "later children must retain the authenticated digest: {}",
        String::from_utf8_lossy(&result.stdout)
    );
    let style: Value = serde_json::from_slice(
        &fs::read(root.path().join("build/infra-runs/manager-tampered/style.json")).expect("style report"),
    )
    .expect("JSON");
    assert_eq!(style["status"], "error");
}
