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
use serde_json::json;
use tempfile::TempDir;

fn fixture() -> TempDir {
    let project = tempfile::tempdir().expect("temporary project");
    for tool in ["ci", "style", "verify", "tools"] {
        fs::create_dir_all(project.path().join(format!(".infra/{tool}"))).expect("configuration directory");
        fs::write(project.path().join(format!(".infra/{tool}/config.toml")), "schema=1\n").expect("tool configuration");
    }
    fs::write(
        project.path().join(".infra/project.toml"),
        "schema=1\nkind='library'\n[profiles.debug]\nconfigure='debug'\nbuild='debug'\ntest='debug'\n",
    )
    .expect("project configuration");
    configure(&project, "");
    project
}
fn configure(project: &TempDir, extra: &str) {
    fs::write(
        project.path().join(".infra/ci/config.toml"),
        format!("schema=1\ntasks=['style']\nprimary_profile='debug'\n{extra}\n"),
    )
    .expect("CI configuration");
}
fn invoke(project: &TempDir, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_cpp-infra-ci"))
        .arg("--project")
        .arg(project.path())
        .args(args)
        .output()
        .expect("invoke CI")
}
fn rejected(project: &TempDir, args: &[&str], diagnostic: &str) {
    let output = invoke(project, args);
    assert_eq!(
        output.status.code(),
        Some(2),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains(diagnostic),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
#[test]
fn test_project_roots_must_exist_and_cannot_overlap() {
    for roots in ["['source','source/nested']", "['source','source']", "['.','source']"] {
        let project = fixture();
        fs::create_dir_all(project.path().join("source/nested")).expect("source roots");
        let path = project.path().join(".infra/project.toml");
        let original = fs::read_to_string(&path).expect("project config");
        fs::write(
            &path,
            original.replace("kind='library'", &format!("kind='library'\nsource_roots={roots}")),
        )
        .expect("overlapping roots");
        rejected(&project, &["plan"], "overlapping project roots");
    }
    let project = fixture();
    let path = project.path().join(".infra/project.toml");
    let original = fs::read_to_string(&path).expect("project config");
    fs::write(
        &path,
        original.replace("kind='library'", "kind='library'\nsource_roots=['missing']"),
    )
    .expect("missing root");
    rejected(&project, &["plan"], "invalid project input");
}
#[test]
fn test_hook_default_prerequisites_include_all_selected_quality_tasks() {
    let project = fixture();
    fs::write(project.path().join("project-hook"), "fixture executable").expect("hook");
    configure(
        &project,
        "[[hooks]]\nid='custom'\nexecutable='project-hook'\nargs=['argument']\nrequired=false",
    );
    let output = invoke(&project, &["plan"]);
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let plan: Value = serde_json::from_slice(&output.stdout).expect("plan");
    let nodes = plan["nodes"].as_array().expect("nodes");
    let hook = nodes.iter().find(|node| node["id"] == "custom").expect("hook node");
    assert_eq!(hook["needs"], json!(["doctor-style", "style"]));
    assert_eq!(hook["resource_locks"], json!(["project-hook"]));
    assert_eq!(hook["required"], false);
    let output = invoke(&project, &["plan", "--only", "style"]);
    let plan: Value = serde_json::from_slice(&output.stdout).expect("selected plan");
    assert!(
        !plan["nodes"]
            .as_array()
            .expect("nodes")
            .iter()
            .any(|node| node["id"] == "custom")
    );
}
#[test]
fn test_unknown_matrix_selection_and_execution_task_are_configuration_errors() {
    let project = fixture();
    fs::write(
        project.path().join(".infra/ci/config.toml"),
        "schema=1\ntasks=['verify']\nprimary_profile='debug'\n",
    )
    .expect("verification configuration");
    rejected(
        &project,
        &["matrix", "run", "--check", "absent"],
        "unknown matrix check: absent",
    );
    configure(&project, "");
    fs::create_dir(project.path().join("build")).expect("build directory");
    let output = invoke(&project, &["plan"]);
    let plan: Value = serde_json::from_slice(&output.stdout).expect("plan");
    let manifest = project.path().join("build/manifest.json");
    fs::write(&manifest, &output.stdout).expect("manifest");
    let report = project
        .path()
        .join("build/infra-runs")
        .join(plan["run_id"].as_str().expect("run ID"));
    rejected(
        &project,
        &[
            "execute",
            "--manifest",
            manifest.to_str().expect("manifest path"),
            "--task-ids",
            "absent",
            "--output-dir",
            report.to_str().expect("report path"),
        ],
        "unknown task id: absent",
    );
}
#[test]
fn test_legacy_lock_pins_tool_identity_and_rejects_bad_protocol() {
    let project = fixture();
    let revision = "a".repeat(40);
    let lock = json!({"schema":1,"protocol_major":1,"bundle_id":"locked-fixture","tools":[{"name":"cpp-infra-tools","version":"0.1.0","revision":revision},{"name":"cpp-infra-style","version":"0.1.0","revision":revision}]});
    let path = project.path().join(".infra/tools/tools.lock.json");
    fs::write(&path, serde_json::to_vec(&lock).expect("lock bytes")).expect("lock");
    let output = invoke(&project, &["plan"]);
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let plan: Value = serde_json::from_slice(&output.stdout).expect("pinned plan");
    assert_eq!(plan["bundle_id"], "locked-fixture");
    for node in plan["nodes"]
        .as_array()
        .expect("nodes")
        .iter()
        .filter(|node| node["tool"] != "internal")
    {
        assert_eq!(node["revision"], revision);
        assert_eq!(node["version"], "0.1.0");
    }
    let mut invalid = lock.clone();
    invalid["protocol_major"] = json!(2);
    fs::write(&path, serde_json::to_vec(&invalid).expect("invalid bytes")).expect("invalid lock");
    rejected(&project, &["plan"], "unsupported snapshot protocol");
    fs::write(&path, "{").expect("broken JSON");
    rejected(&project, &["plan"], "EOF while parsing");
}
#[test]
fn test_resource_budgets_and_task_configuration_are_checked_before_planning() {
    for setting in ["task_jobs=0", "matrix_max_parallel=0", "timeout_seconds=0"] {
        let project = fixture();
        configure(&project, setting);
        rejected(&project, &["plan"], "invalid task/matrix resource budget");
    }
    for tasks in ["['style','style']", "['unsupported']"] {
        let project = fixture();
        fs::write(
            project.path().join(".infra/ci/config.toml"),
            format!("schema=1\ntasks={tasks}\nprimary_profile='debug'\n"),
        )
        .expect("invalid tasks");
        rejected(&project, &["plan"], "unknown or duplicate task");
    }
    let project = fixture();
    fs::write(
        project.path().join(".infra/ci/config.toml"),
        "schema=1\ntasks=['coverage']\nprimary_profile='debug'\n",
    )
    .expect("missing task config");
    rejected(&project, &["plan"], "task coverage has no configuration");
    fs::write(
        project.path().join(".infra/ci/config.toml"),
        "schema=1\ntasks=[]\nprimary_profile='debug'\n",
    )
    .expect("no tasks");
    rejected(&project, &["plan"], "no enabled tasks");
}
#[test]
fn test_matrix_environment_and_suite_bindings_are_validated() {
    for (entry, diagnostic) in [
        (
            "runner=''\nos='linux'\narch='x86_64'\nsuites=['test']",
            "invalid matrix entry",
        ),
        (
            "runner='host'\nos='other'\narch='x86_64'\nsuites=['test']",
            "invalid matrix entry",
        ),
        (
            "runner='host'\nos='linux'\narch='other'\nsuites=['test']",
            "invalid matrix entry",
        ),
        ("runner='host'\nos='linux'\narch='x86_64'\nsuites=[]", "invalid suites"),
        (
            "runner='host'\nos='linux'\narch='x86_64'\nsuites=['test','test']",
            "invalid suites",
        ),
        (
            "runner='host'\nos='linux'\narch='x86_64'\nsuites=['other']",
            "invalid suites",
        ),
    ] {
        let project = fixture();
        configure(&project, &format!("[[matrix]]\nid='matrix'\nprofile='debug'\n{entry}"));
        rejected(&project, &["plan"], diagnostic);
    }
}
#[test]
fn test_empty_preset_bindings_and_zero_hook_timeout_are_rejected() {
    for binding in [
        "configure=''\nbuild='debug'",
        "configure='debug'\nbuild=''",
        "configure='debug'\nbuild='debug'\ntest=''",
    ] {
        let project = fixture();
        fs::write(
            project.path().join(".infra/project.toml"),
            format!("schema=1\nkind='library'\n[profiles.debug]\n{binding}\n"),
        )
        .expect("empty binding");
        rejected(&project, &["plan"], "empty preset binding");
    }
    let project = fixture();
    fs::write(project.path().join("hook"), "fixture").expect("hook");
    configure(&project, "[[hooks]]\nid='hook'\nexecutable='hook'\ntimeout_seconds=0");
    rejected(&project, &["plan"], "zero hook timeout");
}
#[test]
fn test_malformed_existing_report_is_not_a_source_exclusion() {
    let project = fixture();
    fs::write(project.path().join("result.json"), "{").expect("malformed business file");
    rejected(
        &project,
        &[
            "--report",
            project.path().join("result.json").to_str().expect("report path"),
            "plan",
        ],
        "existing report path is not a public report: result.json",
    );
    assert_eq!(
        fs::read_to_string(project.path().join("result.json")).expect("preserved file"),
        "{"
    );
}
