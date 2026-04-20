use std::io::Read;
use std::path::Path;
use std::process::{Command, Output};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;
use tempfile::TempDir;

fn setup_repo() -> (TempDir, String) {
    let temp = tempfile::tempdir().unwrap();
    Command::new("git")
        .args(["init"])
        .current_dir(temp.path())
        .output()
        .unwrap();
    let db_path = temp.path().join("test.sqlite3");
    (temp, db_path.to_string_lossy().to_string())
}

fn setup_non_repo() -> TempDir {
    tempfile::tempdir().unwrap()
}

fn bin() -> assert_cmd::Command {
    assert_cmd::Command::cargo_bin("issuectl").unwrap()
}

fn output(dir: &Path, db_path: &str, args: &[&str]) -> Output {
    let mut cmd = bin();
    cmd.current_dir(dir)
        .env("ISSUECTL_DB_PATH", db_path)
        .args(args)
        .output()
        .unwrap()
}

fn success_output(dir: &Path, db_path: &str, args: &[&str]) -> Output {
    let output = output(dir, db_path, args);
    assert!(
        output.status.success(),
        "command failed: {:?}\nstdout:\n{}\nstderr:\n{}",
        args,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn json_output(dir: &Path, db_path: &str, args: &[&str]) -> Value {
    serde_json::from_slice(&success_output(dir, db_path, args).stdout).unwrap()
}

fn stderr_string(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).to_string()
}

fn stdout_string(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).to_string()
}

fn default_item_prefix(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("project")
        .chars()
        .find(|ch| ch.is_ascii_alphabetic())
        .map(|ch| ch.to_ascii_uppercase().to_string())
        .unwrap_or_else(|| "P".to_string())
}

fn item_id(path: &Path, number: usize) -> String {
    format!("{}-{number}", default_item_prefix(path))
}

#[test]
fn no_active_project_errors_are_reported_outside_repo() {
    let dir = setup_non_repo();
    let db_path = dir.path().join("test.sqlite3");
    for args in [vec!["project", "show"], vec!["item", "list"], vec!["next"]] {
        let out = output(dir.path(), &db_path.to_string_lossy(), &args);
        assert_eq!(out.status.code(), Some(1));
        assert!(stderr_string(&out).contains("no active project found"));
    }
}

#[test]
fn unknown_ids_and_missing_relations_report_validation_errors() {
    let (repo, db_path) = setup_repo();
    json_output(repo.path(), &db_path, &["--json", "init"]);
    let first_id = item_id(repo.path(), 1);
    let missing_id = item_id(repo.path(), 99);
    success_output(
        repo.path(),
        &db_path,
        &["item", "create", "--title", "Task"],
    );

    for args in [
        vec!["item", "show", &missing_id],
        vec!["history", "show", &missing_id],
        vec!["history", "command", "CMD-99"],
        vec!["undo", "CMD-99"],
        vec!["item", "create", "--title", "Child", "--parent", &missing_id],
        vec!["item", "block", &first_id, "--by", &missing_id],
    ] {
        let out = output(repo.path(), &db_path, &args);
        assert_eq!(out.status.code(), Some(1), "args: {:?}", args);
        assert!(stderr_string(&out).contains("unknown"), "args: {:?}", args);
    }
}

#[test]
fn init_is_idempotent_for_existing_project() {
    let (repo, db_path) = setup_repo();

    json_output(repo.path(), &db_path, &["--json", "init"]);
    json_output(repo.path(), &db_path, &["--json", "init"]);

    let projects = json_output(repo.path(), &db_path, &["--json", "project", "list"]);
    assert_eq!(projects["projects"].as_array().unwrap().len(), 1);

    let commands = json_output(repo.path(), &db_path, &["--json", "history", "list"]);
    assert_eq!(commands["commands"].as_array().unwrap().len(), 1);
    assert_eq!(commands["commands"][0]["action"], "project.init");
}

#[test]
fn project_use_can_be_undone_cleanly() {
    let (repo, db_path) = setup_repo();
    let other = setup_non_repo();

    json_output(repo.path(), &db_path, &["--json", "init"]);
    json_output(
        other.path(),
        &db_path,
        &["--json", "project", "use", "PRJ-1"],
    );

    let undone = json_output(other.path(), &db_path, &["--json", "undo", "CMD-2"]);
    assert_eq!(undone["reversed_command"], "CMD-2");

    let out = output(other.path(), &db_path, &["project", "show"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr_string(&out).contains("no active project found"));
}

#[test]
fn duplicate_block_and_unblock_are_idempotent() {
    let (repo, db_path) = setup_repo();
    json_output(repo.path(), &db_path, &["--json", "init"]);
    success_output(repo.path(), &db_path, &["item", "create", "--title", "A"]);
    success_output(repo.path(), &db_path, &["item", "create", "--title", "B"]);
    let first_id = item_id(repo.path(), 1);
    let second_id = item_id(repo.path(), 2);

    json_output(
        repo.path(),
        &db_path,
        &["--json", "item", "block", &second_id, "--by", &first_id],
    );
    json_output(
        repo.path(),
        &db_path,
        &["--json", "item", "block", &second_id, "--by", &first_id],
    );
    let blockers = json_output(
        repo.path(),
        &db_path,
        &["--json", "item", "blockers", &second_id],
    );
    assert_eq!(blockers["blockers"].as_array().unwrap().len(), 1);

    json_output(
        repo.path(),
        &db_path,
        &["--json", "item", "unblock", &second_id, "--by", &first_id],
    );
    json_output(
        repo.path(),
        &db_path,
        &["--json", "item", "unblock", &second_id, "--by", &first_id],
    );
    let blockers_after = json_output(
        repo.path(),
        &db_path,
        &["--json", "item", "blockers", &second_id],
    );
    assert!(blockers_after["blockers"].as_array().unwrap().is_empty());
}

#[test]
fn human_outputs_include_expected_text() {
    let (repo, db_path) = setup_repo();
    success_output(repo.path(), &db_path, &["init"]);
    let first_id = item_id(repo.path(), 1);
    let create = success_output(
        repo.path(),
        &db_path,
        &["item", "create", "--title", "Task"],
    );
    assert!(stdout_string(&create).contains(&format!("Created item: {first_id} Task")));
    assert!(stdout_string(&create).contains("project=PRJ-1"));

    let explained = success_output(repo.path(), &db_path, &["project", "show", "--explain"]);
    let explained_stdout = stdout_string(&explained);
    assert!(explained_stdout.contains("resolved_by=repo_root"));
    assert!(explained_stdout.contains("created=false"));

    let show = success_output(repo.path(), &db_path, &["item", "show", &first_id]);
    let show_stdout = stdout_string(&show);
    assert!(show_stdout.contains(&format!("{first_id}: Task")));
    assert!(show_stdout.contains("status=todo priority=medium ready=false"));

    success_output(repo.path(), &db_path, &["item", "ready", &first_id]);
    let next = success_output(repo.path(), &db_path, &["next"]);
    assert!(stdout_string(&next).contains(&format!("{first_id}\tmedium\tTask")));

    let tree = success_output(repo.path(), &db_path, &["item", "tree"]);
    assert!(stdout_string(&tree).contains(&format!("{first_id} [todo ready=true] Task")));
}

#[test]
fn human_item_create_ready_flag_sets_ready_state() {
    let (repo, db_path) = setup_repo();
    success_output(repo.path(), &db_path, &["init"]);

    let create = success_output(
        repo.path(),
        &db_path,
        &["item", "create", "--title", "Task", "--ready"],
    );

    assert!(stdout_string(&create).contains("status=todo priority=medium ready=true"));
}

#[test]
fn history_list_is_limited_to_fifty_entries() {
    let (repo, db_path) = setup_repo();
    json_output(repo.path(), &db_path, &["--json", "init"]);

    for i in 0..55 {
        let title = format!("Task-{i}");
        success_output(
            repo.path(),
            &db_path,
            &["item", "create", "--title", &title],
        );
    }

    let history = json_output(repo.path(), &db_path, &["--json", "history", "list"]);
    assert_eq!(history["commands"].as_array().unwrap().len(), 50);
    assert_eq!(history["commands"][0]["public_id"], "CMD-56");
}

#[test]
fn next_limit_and_remaining_item_filters_work() {
    let (repo, db_path) = setup_repo();
    json_output(repo.path(), &db_path, &["--json", "init"]);
    let first_id = item_id(repo.path(), 1);
    let second_id = item_id(repo.path(), 2);
    let third_id = item_id(repo.path(), 3);
    success_output(repo.path(), &db_path, &["item", "create", "--title", "One"]);
    success_output(repo.path(), &db_path, &["item", "create", "--title", "Two"]);
    success_output(
        repo.path(),
        &db_path,
        &["item", "create", "--title", "Three"],
    );
    success_output(repo.path(), &db_path, &["item", "ready", &first_id]);
    success_output(repo.path(), &db_path, &["item", "ready", &second_id]);
    success_output(
        repo.path(),
        &db_path,
        &["item", "status", &third_id, "cancelled"],
    );

    let ready_true = json_output(
        repo.path(),
        &db_path,
        &["--json", "item", "list", "--ready", "true"],
    );
    assert_eq!(ready_true["items"].as_array().unwrap().len(), 2);

    let status_cancelled = json_output(
        repo.path(),
        &db_path,
        &["--json", "item", "list", "--status", "cancelled"],
    );
    assert_eq!(status_cancelled["items"].as_array().unwrap().len(), 1);
    assert_eq!(status_cancelled["items"][0]["public_id"], third_id);

    let blocked_false = json_output(
        repo.path(),
        &db_path,
        &["--json", "item", "list", "--blocked", "false"],
    );
    assert_eq!(blocked_false["items"].as_array().unwrap().len(), 3);

    let next = json_output(repo.path(), &db_path, &["--json", "next", "--limit", "1"]);
    assert_eq!(next["items"].as_array().unwrap().len(), 1);
}

#[test]
fn unknown_project_filter_is_rejected() {
    let (repo, db_path) = setup_repo();
    json_output(repo.path(), &db_path, &["--json", "init"]);
    let out = output(
        repo.path(),
        &db_path,
        &["item", "list", "--project", "PRJ-99"],
    );
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr_string(&out).contains("unknown project id"));
}

#[test]
fn invalid_enum_values_are_rejected_by_clap() {
    let (repo, db_path) = setup_repo();
    json_output(repo.path(), &db_path, &["--json", "init"]);
    let first_id = item_id(repo.path(), 1);
    success_output(
        repo.path(),
        &db_path,
        &["item", "create", "--title", "Task"],
    );

    let bad_status = output(repo.path(), &db_path, &["item", "status", &first_id, "bogus"]);
    assert_eq!(bad_status.status.code(), Some(2));
    assert!(stderr_string(&bad_status).contains("invalid value"));

    let bad_priority = output(
        repo.path(),
        &db_path,
        &["item", "create", "--title", "Other", "--priority", "bogus"],
    );
    assert_eq!(bad_priority.status.code(), Some(2));
    assert!(stderr_string(&bad_priority).contains("invalid value"));
}

#[test]
fn invalid_project_prefixes_are_rejected() {
    let (repo, db_path) = setup_repo();
    json_output(repo.path(), &db_path, &["--json", "init"]);

    let bad_prefix = output(
        repo.path(),
        &db_path,
        &["project", "update", "PRJ-1", "--prefix", "bad-prefix"],
    );
    assert_eq!(bad_prefix.status.code(), Some(1));
    assert!(stderr_string(&bad_prefix).contains("item prefix"));
}

#[test]
fn next_wait_blocks_until_item_becomes_ready() {
    let (repo, db_path) = setup_repo();
    json_output(repo.path(), &db_path, &["--json", "init"]);
    let first_id = item_id(repo.path(), 1);
    success_output(
        repo.path(),
        &db_path,
        &["item", "create", "--title", "Waiting Task"],
    );

    let mut child = Command::new(assert_cmd::cargo::cargo_bin("issuectl"))
        .current_dir(repo.path())
        .env("ISSUECTL_DB_PATH", &db_path)
        .args(["--json", "next", "--wait"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();

    thread::sleep(Duration::from_millis(350));
    success_output(repo.path(), &db_path, &["item", "ready", &first_id]);

    let start = Instant::now();
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success());
            break;
        }
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "wait command did not complete"
        );
        thread::sleep(Duration::from_millis(50));
    }

    let mut stdout = String::new();
    child
        .stdout
        .take()
        .unwrap()
        .read_to_string(&mut stdout)
        .unwrap();
    let json: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(json["items"][0]["public_id"], first_id);
}
