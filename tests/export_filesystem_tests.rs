// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================

use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::process::Output;

use serde_json::Value;
use tempfile::TempDir;

fn fixture() -> TempDir {
    let root = tempfile::tempdir().expect("temporary project");
    fs::create_dir_all(root.path().join(".infra/ci")).expect("CI directory");
    fs::create_dir_all(root.path().join(".infra/style")).expect("style directory");
    fs::write(root.path().join(".infra/project.toml"), "schema=1\nkind='library'\nsource_roots=[]\ntest_roots=[]\ngenerated_roots=[]\nthird_party_roots=[]\n[profiles.debug]\nconfigure='debug'\nbuild='debug'\ntest='debug'\n").expect("project configuration");
    fs::write(root.path().join(".infra/style/config.toml"), "schema=1\n").expect("style configuration");
    fs::write(
        root.path().join(".infra/ci/config.toml"),
        "schema=1\ntasks=['style']\nprimary_profile='debug'\n",
    )
    .expect("CI configuration");
    root
}

fn invoke(root: &TempDir, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_cpp-infra-ci"))
        .arg("--project")
        .arg(root.path())
        .args(args)
        .output()
        .expect("invoke public CLI")
}

fn assert_code(output: &Output, expected: i32) {
    assert_eq!(
        output.status.code(),
        Some(expected),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn export(root: &TempDir, input: &Path, output: &Path, ids: &str, missing: bool) -> Output {
    let mut args = vec![
        "export",
        "--input-dir",
        input.to_str().expect("input"),
        "--output-dir",
        output.to_str().expect("output"),
        "--task-ids",
        ids,
    ];
    if missing {
        args.push("--allow-missing");
    }
    invoke(root, &args)
}

#[cfg(unix)]
fn completed_run(root: &TempDir) -> PathBuf {
    completed_run_with_empty_directory(root, false)
}

#[cfg(unix)]
fn completed_run_with_empty_directory(root: &TempDir, empty: bool) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let bin = tempfile::tempdir().expect("protocol executables");
    // These children implement only the public report protocol; no CMake is run.
    let script = r#"#!/usr/bin/env python3
import sys,json,datetime,hashlib,struct,os
from pathlib import Path
a=sys.argv[1:]
def get(k): return a[a.index(k)+1]
c=json.load(open(get('--run-context')))
r={k:c[k] for k in ['schema','run_id','task_id','bundle_id','project','report_root','source_digest','config_digest']}
r.update(tool={'name':Path(sys.argv[0]).name,'version':'0.1.0','revision':None},source_revision=None,status='passed',started_at=datetime.datetime.now(datetime.timezone.utc).isoformat(),duration_ms=0,external_versions={},commands=[],diagnostics=[],artifacts=[])
if c['task_id']=='style':
 root=Path(c['report_root']);site=root/'evidence/site';site.mkdir(parents=True)
 (root/'evidence/tool.log').write_bytes(b'child log')
 if os.environ.get('CI_FIXTURE_EMPTY')=='1':digest=hashlib.sha256(b'').hexdigest()
 else:
  (site/'index.html').write_bytes(b'site')
  name=b'index.html';data=b'site';digest=hashlib.sha256(struct.pack('<Q',len(name))+name+struct.pack('<Q',len(data))+data).hexdigest()
 r['artifacts']=[dict(kind='html',path='evidence/site',sha256=digest,required=True)]
 r['commands']=[dict(program='fixture',args=[],cwd=c['project'],exit_code=0,log='evidence/tool.log',duration_ms=0,env={})]
json.dump(r,open(get('--report'),'w'))
"#;
    for tool in ["tools", "style"] {
        let path = bin.path().join(format!("cpp-infra-{tool}"));
        fs::write(&path, script).expect("protocol executable");
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).expect("executable permissions");
    }
    let output = Command::new(env!("CARGO_BIN_EXE_cpp-infra-ci"))
        .arg("--project")
        .arg(root.path())
        .arg("check")
        .env("CPP_INFRA_BIN_DIR", bin.path())
        .env("CI_FIXTURE_EMPTY", if empty { "1" } else { "0" })
        .output()
        .expect("run fixture children");
    assert_code(&output, 0);
    let summary: Value = serde_json::from_slice(&output.stdout).expect("summary JSON");
    PathBuf::from(summary["report_root"].as_str().expect("run root"))
}

#[cfg(unix)]
#[test]
fn test_export_preserves_empty_directory_artifact_for_import() {
    let root = fixture();
    let input = completed_run_with_empty_directory(&root, true);
    let destination = root.path().join("build/exported");
    assert_code(&export(&root, &input, &destination, "doctor-style,style", false), 0);
    assert!(
        destination.join("evidence/site").is_dir(),
        "a valid directory artifact must survive portable export even when empty"
    );
    let restored = root.path().join("build/restored");
    assert_code(&export(&root, &destination, &restored, "doctor-style,style", false), 0);
    assert!(restored.join("evidence/site").is_dir());
}

#[cfg(unix)]
#[test]
fn test_export_empty_directory_conflict_is_rejected_before_any_copy() {
    let root = fixture();
    let input = completed_run_with_empty_directory(&root, true);
    let destination = root.path().join("build/exported");
    fs::create_dir_all(destination.join("evidence")).expect("destination parent");
    let business = destination.join("evidence/site");
    fs::write(&business, "business data").expect("directory name occupied by business file");
    let result = export(&root, &input, &destination, "doctor-style,style", false);
    assert_code(&result, 2);
    assert_eq!(fs::read(business).expect("business preserved"), b"business data");
    assert!(!destination.join("doctor-style.json").exists());
    assert!(!destination.join("evidence/tool.log").exists());
}

#[cfg(unix)]
#[test]
fn test_export_roundtrip_keeps_recursive_directory_and_child_command_logs() {
    let root = fixture();
    let input = completed_run(&root);
    let portable = tempfile::tempdir().expect("portable download");
    let output = portable.path().join("job");
    assert_code(&export(&root, &input, &output, "doctor-style,style", false), 0);
    for name in [
        "doctor-style.json",
        "style.json",
        "style-tool.json",
        "style.log",
        "evidence/site/index.html",
        "evidence/tool.log",
    ] {
        assert_eq!(
            fs::read(input.join(name)).expect("original evidence"),
            fs::read(output.join(name)).expect("exported evidence"),
            "{name}"
        );
    }
    assert!(!output.join("run-manifest.json").exists());
    assert!(!output.join("style-context.json").exists());
    assert_code(&export(&root, &input, &output, "doctor-style,style", false), 0);
    let restored = portable.path().join("restored");
    assert_code(&export(&root, &output, &restored, "doctor-style,style", false), 0);
    assert_eq!(
        fs::read(restored.join("evidence/site/index.html")).expect("roundtrip directory"),
        b"site"
    );
}

#[cfg(unix)]
#[test]
fn test_export_checks_every_destination_before_copying_reports() {
    let root = fixture();
    let input = completed_run(&root);
    for directory_conflict in [false, true] {
        let destination = tempfile::tempdir().expect("destination");
        let conflict = destination.path().join("style.json");
        if directory_conflict {
            fs::create_dir(&conflict).expect("business directory");
        } else {
            fs::write(&conflict, "business bytes").expect("business file");
        }
        let output = export(&root, &input, destination.path(), "doctor-style,style", false);
        assert_code(&output, 2);
        assert!(String::from_utf8_lossy(&output.stderr).contains("conflicting exported artifact"));
        assert!(
            !destination.path().join("doctor-style.json").exists(),
            "preflight failure must not publish earlier reports"
        );
        assert!(!destination.path().join("evidence").exists());
        if !directory_conflict {
            assert_eq!(fs::read(&conflict).expect("business preserved"), b"business bytes");
        }
    }
}

#[cfg(unix)]
#[test]
fn test_export_imports_reports_into_another_runner_for_final_execution() {
    let producer = fixture();
    let input = completed_run(&producer);
    let consumer = fixture();
    let plan_path = consumer.path().join("build/portable-plan.json");
    fs::create_dir_all(plan_path.parent().expect("build directory")).expect("consumer build");
    let bytes = fs::read(input.join("run-manifest.json")).expect("producer manifest");
    fs::write(&plan_path, &bytes).expect("portable manifest");
    let plan: Value = serde_json::from_slice(&bytes).expect("manifest JSON");
    let destination = consumer
        .path()
        .join("build/infra-runs")
        .join(plan["run_id"].as_str().expect("shared run ID"));
    assert_code(&export(&producer, &input, &destination, "doctor-style,style", false), 0);
    let result = invoke(
        &consumer,
        &[
            "execute",
            "--manifest",
            "build/portable-plan.json",
            "--task-ids",
            "quality-summary,final-summary",
            "--output-dir",
            destination.to_str().expect("consumer run"),
            "--require-existing-dependencies",
        ],
    );
    assert_code(&result, 0);
    let summary: Value = serde_json::from_slice(&result.stdout).expect("consumer summary");
    assert_eq!(summary["status"], "passed");
    assert_eq!(summary["data"]["complete"], true);
    assert_eq!(
        summary["project"],
        serde_json::json!(consumer.path().canonicalize().expect("consumer"))
    );
    assert_ne!(producer.path(), consumer.path());
}

#[cfg(unix)]
#[test]
fn test_export_writes_public_operation_report_and_allows_missing_selected_task() {
    let root = fixture();
    let input = completed_run(&root);
    let destination = root.path().join("build/exported");
    let report = root.path().join("build/export-report.json");
    let output = invoke(
        &root,
        &[
            "--report",
            report.to_str().expect("public report"),
            "export",
            "--input-dir",
            input.to_str().expect("input"),
            "--output-dir",
            destination.to_str().expect("destination"),
            "--task-ids",
            "style,absent",
            "--allow-missing",
        ],
    );
    assert_code(&output, 0);
    let value: Value = serde_json::from_slice(&fs::read(&report).expect("export report")).expect("report JSON");
    assert_eq!(value["status"], "passed");
    assert_eq!(value["data"]["operation"], "export");
    assert_eq!(
        value["data"]["exported_task_ids"],
        serde_json::json!(["absent", "style"])
    );
    assert!(destination.join("style.json").is_file());
    assert!(!destination.join("absent.json").exists());
}

#[cfg(unix)]
#[test]
fn test_export_missing_later_task_does_not_publish_earlier_valid_task() {
    let root = fixture();
    let input = completed_run(&root);
    let destination = root.path().join("build/exported");
    assert_code(
        &export(&root, &input, &destination, "doctor-style,zzz-missing", false),
        3,
    );
    assert!(!destination.exists());
}

#[cfg(unix)]
#[test]
fn test_export_rejects_corrupt_recursive_artifact_before_creating_output() {
    let root = fixture();
    let input = completed_run(&root);
    fs::write(input.join("evidence/site/index.html"), "tampered site").expect("tamper evidence");
    let destination = root.path().join("build/exported");
    let output = export(&root, &input, &destination, "doctor-style,style", false);
    assert_code(&output, 2);
    assert!(String::from_utf8_lossy(&output.stderr).contains("export artifact digest mismatch"));
    assert!(!destination.exists());
}

#[cfg(unix)]
#[test]
fn test_export_rejects_overlapping_input_and_output_trees() {
    let root = fixture();
    let input = completed_run(&root);
    for output in [
        input.clone(),
        input.join("nested"),
        input.parent().expect("run parent").to_path_buf(),
    ] {
        let result = export(&root, &input, &output, "style", false);
        assert_code(&result, 2);
        assert!(String::from_utf8_lossy(&result.stderr).contains("must be disjoint"));
    }
    assert!(!input.join("nested").exists());
}

#[test]
fn test_export_missing_input_is_io_error_without_creating_destination() {
    let root = fixture();
    let output = root.path().join("build/exported");
    assert_code(&export(&root, &root.path().join("absent"), &output, "style", true), 3);
    assert!(!output.exists());
}

#[test]
fn test_export_allow_missing_does_not_invent_reports_or_require_cache() {
    let root = fixture();
    let input = tempfile::tempdir().expect("empty report input");
    let output = root.path().join("build/exported");
    assert_code(&export(&root, input.path(), &output, "doctor-style,style", true), 0);
    assert!(!output.exists());
    assert!(!root.path().join("build").exists());
    assert_code(&export(&root, input.path(), &output, "style", false), 3);
}

#[test]
fn test_export_rejects_duplicate_and_traversal_task_ids_without_writes() {
    let root = fixture();
    let input = tempfile::tempdir().expect("empty report input");
    let output = root.path().join("build/exported");
    for ids in ["style,style", "../style", "", "style,../other"] {
        assert_code(&export(&root, input.path(), &output, ids, true), 2);
        assert!(!output.exists());
    }
}

#[cfg(unix)]
#[test]
fn test_export_blocked_destination_parent_is_io_error_and_preserves_business_file() {
    let root = fixture();
    let input = completed_run(&root);
    let blocker = root.path().join("build/business");
    fs::write(&blocker, "business data").expect("non-directory parent");
    assert_code(&export(&root, &input, &blocker.join("exported"), "style", false), 3);
    assert_eq!(fs::read(&blocker).expect("business preserved"), b"business data");
}

fn planned(root: &TempDir) -> (PathBuf, PathBuf) {
    let manifest = root.path().join("build/original-plan.json");
    assert_code(
        &invoke(root, &["plan", "--output", manifest.to_str().expect("manifest")]),
        0,
    );
    let plan: Value = serde_json::from_slice(&fs::read(&manifest).expect("manifest bytes")).expect("manifest JSON");
    let output = root
        .path()
        .join("build/infra-runs")
        .join(plan["run_id"].as_str().expect("run ID"));
    (manifest, output)
}

fn execute(root: &TempDir, manifest: &Path, output: &Path) -> Output {
    invoke(
        root,
        &[
            "execute",
            "--manifest",
            manifest.to_str().expect("manifest"),
            "--task-ids",
            "style",
            "--output-dir",
            output.to_str().expect("run root"),
            "--require-existing-dependencies",
        ],
    )
}

#[test]
fn test_execute_requires_existing_prerequisite_report_before_spawning() {
    let root = fixture();
    let (manifest, output) = planned(&root);
    let result = execute(&root, &manifest, &output);
    assert_code(&result, 2);
    assert!(String::from_utf8_lossy(&result.stderr).contains("lacks required prerequisite reports"));
    assert!(!output.join("style-context.json").exists());
    assert!(!output.join("run-manifest.json").exists());
}

#[test]
fn test_execute_preserves_unmanaged_fixed_run_targets() {
    for name in [
        "style.json",
        "style-context.json",
        "style-tool.json",
        "style.log",
        "style-build-context.json",
    ] {
        let root = fixture();
        let (manifest, output) = planned(&root);
        fs::create_dir_all(&output).expect("run directory");
        let business = output.join(name);
        fs::write(&business, "business bytes").expect("unmanaged run target");
        let result = execute(&root, &manifest, &output);
        assert_code(&result, 2);
        assert!(String::from_utf8_lossy(&result.stderr).contains("unmanaged"), "{name}");
        assert_eq!(fs::read(&business).expect("preserved target"), b"business bytes");
        assert!(!output.join("run-manifest.json").exists());
    }
}

#[test]
fn test_execute_rejects_existing_run_file_with_another_identity() {
    let root = fixture();
    let (manifest, output) = planned(&root);
    fs::create_dir_all(&output).expect("run directory");
    let business = output.join("style-context.json");
    let bytes = br#"{"schema":1,"run_id":"other-run","task_id":"style","bundle_id":"development"}"#;
    fs::write(&business, bytes).expect("foreign context");
    let result = execute(&root, &manifest, &output);
    assert_code(&result, 2);
    assert!(String::from_utf8_lossy(&result.stderr).contains("identity mismatch"));
    assert_eq!(fs::read(business).expect("foreign context preserved"), bytes);
}

#[test]
fn test_execute_reports_missing_and_malformed_manifest_with_distinct_categories() {
    let root = fixture();
    let manifest = root.path().join("build/missing.json");
    let output = root.path().join("build/output");
    assert_code(&execute(&root, &manifest, &output), 3);
    fs::create_dir_all(manifest.parent().expect("build directory")).expect("build");
    fs::write(&manifest, "invalid JSON").expect("malformed manifest");
    let result = execute(&root, &manifest, &output);
    assert_code(&result, 2);
    assert!(String::from_utf8_lossy(&result.stderr).contains("invalid execution manifest"));
    assert!(!output.exists());
}

#[test]
fn test_execute_rejects_stale_source_without_publishing_manifest() {
    let root = fixture();
    let (manifest, output) = planned(&root);
    fs::write(root.path().join("new.cpp"), "int new_source;").expect("source change");
    let result = execute(&root, &manifest, &output);
    assert_code(&result, 2);
    assert!(String::from_utf8_lossy(&result.stderr).contains("execution manifest identity mismatch"));
    assert!(!output.exists());
}
