use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

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
    assert_cmd::Command::cargo_bin("issuecli").unwrap()
}

fn output(dir: &Path, db_path: &str, args: &[&str]) -> Output {
    let mut cmd = bin();
    cmd.current_dir(dir)
        .env("ISSUECLI_DB_PATH", db_path)
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
        stdout_string(&output),
        stderr_string(&output)
    );
    output
}

fn json_output(dir: &Path, db_path: &str, args: &[&str]) -> Value {
    let output = success_output(dir, db_path, args);
    serde_json::from_slice(&output.stdout).unwrap()
}

fn stdout_string(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).to_string()
}

fn stderr_string(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).to_string()
}

fn command_ids(dir: &Path, db_path: &str) -> Vec<String> {
    let json = json_output(dir, db_path, &["--json", "history", "list"]);
    json["commands"]
        .as_array()
        .unwrap()
        .iter()
        .map(|command| command["public_id"].as_str().unwrap().to_string())
        .collect()
}

fn item_ids(dir: &Path, db_path: &str) -> Vec<String> {
    let json = json_output(dir, db_path, &["--json", "item", "list"]);
    json["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["public_id"].as_str().unwrap().to_string())
        .collect()
}

fn create_item(dir: &Path, db_path: &str, title: &str) -> Value {
    json_output(
        dir,
        db_path,
        &["--json", "item", "create", "--title", title],
    )
}

fn path_string(path: &Path) -> String {
    fs::canonicalize(path)
        .unwrap_or_else(|_| PathBuf::from(path))
        .to_string_lossy()
        .to_string()
}

#[test]
fn init_requires_git_repository() {
    let dir = setup_non_repo();
    let db_path = dir.path().join("test.sqlite3");
    let output = output(dir.path(), &db_path.to_string_lossy(), &["--json", "init"]);

    assert_eq!(output.status.code(), Some(1));
    let json: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(json["error"]["code"], "validation_error");
    assert!(
        json["error"]["message"]
            .as_str()
            .unwrap()
            .contains("not inside a Git repository")
    );
}

#[test]
fn init_project_show_and_project_list_work() {
    let (repo, db_path) = setup_repo();

    let init = json_output(repo.path(), &db_path, &["--json", "init"]);
    assert_eq!(init["project"]["public_id"], "PRJ-1");
    assert_eq!(
        init["project"]["name"],
        repo.path().file_name().unwrap().to_string_lossy().as_ref()
    );
    assert_eq!(init["project"]["repo_root"], path_string(repo.path()));

    let show = json_output(repo.path(), &db_path, &["--json", "project", "show"]);
    assert_eq!(show["project"]["public_id"], "PRJ-1");

    let list = json_output(repo.path(), &db_path, &["--json", "project", "list"]);
    assert_eq!(list["projects"].as_array().unwrap().len(), 1);
    assert_eq!(list["projects"][0]["public_id"], "PRJ-1");
}

#[test]
fn project_use_allows_non_repo_context_after_init() {
    let (repo, db_path) = setup_repo();
    let other_dir = setup_non_repo();

    json_output(repo.path(), &db_path, &["--json", "init"]);
    let used = json_output(
        other_dir.path(),
        &db_path,
        &["--json", "project", "use", "PRJ-1"],
    );
    assert_eq!(used["project"]["public_id"], "PRJ-1");

    let show = json_output(other_dir.path(), &db_path, &["--json", "project", "show"]);
    assert_eq!(show["project"]["public_id"], "PRJ-1");
}

#[test]
fn project_use_override_wins_over_repo_context_for_reads_and_writes() {
    let (repo_a, db_path) = setup_repo();
    let (repo_b, _) = setup_repo();

    json_output(repo_a.path(), &db_path, &["--json", "init"]);
    let init_b = json_output(repo_b.path(), &db_path, &["--json", "init"]);
    assert_eq!(init_b["project"]["public_id"], "PRJ-2");

    json_output(
        repo_b.path(),
        &db_path,
        &["--json", "project", "update", "PRJ-2", "--prefix", "app"],
    );
    json_output(
        repo_a.path(),
        &db_path,
        &["--json", "project", "use", "PRJ-2"],
    );

    let show = json_output(repo_a.path(), &db_path, &["--json", "project", "show"]);
    assert_eq!(show["project"]["public_id"], "PRJ-2");

    let created = json_output(
        repo_a.path(),
        &db_path,
        &["--json", "item", "create", "--title", "Cross-project item"],
    );
    assert_eq!(created["item"]["public_id"], "APP-1");

    let listed = json_output(repo_a.path(), &db_path, &["--json", "item", "list"]);
    let items = listed["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["public_id"], "APP-1");

    success_output(repo_a.path(), &db_path, &["item", "ready", "APP-1"]);
    let next = json_output(repo_a.path(), &db_path, &["--json", "next"]);
    assert_eq!(next["items"][0]["public_id"], "APP-1");
}

#[test]
fn mutating_item_commands_accept_explicit_project_targeting() {
    let (repo_a, db_path) = setup_repo();
    let (repo_b, _) = setup_repo();

    json_output(repo_a.path(), &db_path, &["--json", "init"]);
    json_output(repo_b.path(), &db_path, &["--json", "init"]);
    json_output(
        repo_b.path(),
        &db_path,
        &["--json", "project", "update", "PRJ-2", "--prefix", "app"],
    );
    json_output(
        repo_a.path(),
        &db_path,
        &["--json", "project", "use", "PRJ-1"],
    );

    let parent = json_output(
        repo_a.path(),
        &db_path,
        &[
            "--json",
            "item",
            "create",
            "--project",
            "PRJ-2",
            "--title",
            "Parent",
        ],
    );
    assert_eq!(parent["item"]["public_id"], "APP-1");

    let child = json_output(
        repo_a.path(),
        &db_path,
        &[
            "--json",
            "item",
            "create",
            "--project",
            "PRJ-2",
            "--title",
            "Child",
        ],
    );
    assert_eq!(child["item"]["public_id"], "APP-2");

    let blocker = json_output(
        repo_a.path(),
        &db_path,
        &[
            "--json",
            "item",
            "create",
            "--project",
            "PRJ-2",
            "--title",
            "Blocker",
        ],
    );
    assert_eq!(blocker["item"]["public_id"], "APP-3");

    json_output(
        repo_a.path(),
        &db_path,
        &[
            "--json",
            "item",
            "update",
            "APP-2",
            "--project",
            "PRJ-2",
            "--title",
            "Child updated",
            "--priority",
            "urgent",
            "--parent",
            "APP-1",
        ],
    );
    json_output(
        repo_a.path(),
        &db_path,
        &["--json", "item", "ready", "APP-2", "--project", "PRJ-2"],
    );
    json_output(
        repo_a.path(),
        &db_path,
        &[
            "--json",
            "item",
            "status",
            "APP-2",
            "in-progress",
            "--project",
            "PRJ-2",
        ],
    );
    json_output(
        repo_a.path(),
        &db_path,
        &[
            "--json",
            "item",
            "block",
            "APP-2",
            "--by",
            "APP-3",
            "--project",
            "PRJ-2",
        ],
    );

    json_output(
        repo_b.path(),
        &db_path,
        &["--json", "project", "use", "PRJ-2"],
    );
    let blocked = json_output(
        repo_b.path(),
        &db_path,
        &["--json", "item", "show", "APP-2"],
    );
    assert_eq!(blocked["item"]["title"], "Child updated");
    assert_eq!(blocked["item"]["priority"], "urgent");
    assert_eq!(blocked["item"]["status"], "in_progress");
    assert_eq!(blocked["item"]["parent_id"], "APP-1");
    assert_eq!(blocked["blockers"][0], "APP-3");

    json_output(
        repo_a.path(),
        &db_path,
        &[
            "--json",
            "item",
            "unblock",
            "APP-2",
            "--by",
            "APP-3",
            "--project",
            "PRJ-2",
        ],
    );
    json_output(
        repo_a.path(),
        &db_path,
        &[
            "--json",
            "item",
            "move",
            "APP-2",
            "--project",
            "PRJ-2",
            "--root",
        ],
    );
    json_output(
        repo_a.path(),
        &db_path,
        &["--json", "item", "unready", "APP-2", "--project", "PRJ-2"],
    );

    let final_item = json_output(
        repo_b.path(),
        &db_path,
        &["--json", "item", "show", "APP-2"],
    );
    assert_eq!(final_item["item"]["parent_id"], Value::Null);
    assert_eq!(final_item["item"]["ready"], false);
    assert!(final_item["blocked_by"].as_array().unwrap().is_empty());
}

#[test]
fn project_use_rejects_unknown_project() {
    let dir = setup_non_repo();
    let db_path = dir.path().join("test.sqlite3");
    let output = output(
        dir.path(),
        &db_path.to_string_lossy(),
        &["project", "use", "PRJ-999"],
    );

    assert_eq!(output.status.code(), Some(1));
    assert!(stderr_string(&output).contains("unknown project id"));
}

#[test]
fn project_specific_prefixes_apply_to_new_items_without_rewriting_existing_ids() {
    let (repo_one, db_path) = setup_repo();
    let (repo_two, _) = setup_repo();

    let init_one = json_output(repo_one.path(), &db_path, &["--json", "init"]);
    assert_eq!(init_one["project"]["item_prefix"], "WI");

    let first_item = json_output(
        repo_one.path(),
        &db_path,
        &["--json", "item", "create", "--title", "Legacy"],
    );
    assert_eq!(first_item["item"]["public_id"], "WI-1");

    let updated_one = json_output(
        repo_one.path(),
        &db_path,
        &["--json", "project", "update", "PRJ-1", "--prefix", "app"],
    );
    assert_eq!(updated_one["project"]["item_prefix"], "APP");

    let prefixed_item = json_output(
        repo_one.path(),
        &db_path,
        &["--json", "item", "create", "--title", "Scoped"],
    );
    assert_eq!(prefixed_item["item"]["public_id"], "APP-2");

    let legacy_show = json_output(
        repo_one.path(),
        &db_path,
        &["--json", "item", "show", "WI-1"],
    );
    assert_eq!(legacy_show["item"]["public_id"], "WI-1");

    let init_two = json_output(repo_two.path(), &db_path, &["--json", "init"]);
    assert_eq!(init_two["project"]["public_id"], "PRJ-2");
    assert_eq!(init_two["project"]["item_prefix"], "WI");

    json_output(
        repo_two.path(),
        &db_path,
        &["--json", "project", "update", "PRJ-2", "--prefix", "ops9"],
    );
    let second_project_item = json_output(
        repo_two.path(),
        &db_path,
        &["--json", "item", "create", "--title", "Ops task"],
    );
    assert_eq!(second_project_item["item"]["public_id"], "OPS9-1");
}

#[test]
fn item_create_show_and_list_filters_cover_defaults_and_fields() {
    let (repo, db_path) = setup_repo();
    json_output(repo.path(), &db_path, &["--json", "init"]);

    let parent = json_output(
        repo.path(),
        &db_path,
        &[
            "--json",
            "item",
            "create",
            "--title",
            "Parent",
            "--description",
            "Parent task",
            "--priority",
            "high",
        ],
    );
    let child = json_output(
        repo.path(),
        &db_path,
        &[
            "--json",
            "item",
            "create",
            "--title",
            "Child",
            "--description",
            "Child task",
            "--priority",
            "low",
            "--parent",
            "WI-1",
        ],
    );

    assert_eq!(parent["item"]["public_id"], "WI-1");
    assert_eq!(parent["item"]["ready"], false);
    assert_eq!(parent["item"]["status"], "todo");
    assert_eq!(parent["item"]["priority"], "high");
    assert_eq!(child["item"]["parent_id"], "WI-1");

    let show = json_output(repo.path(), &db_path, &["--json", "item", "show", "WI-1"]);
    assert_eq!(show["item"]["title"], "Parent");
    assert_eq!(show["children"][0]["public_id"], "WI-2");
    assert!(show["blockers"].as_array().unwrap().is_empty());
    assert!(show["blocked_by"].as_array().unwrap().is_empty());

    let ready_false = json_output(
        repo.path(),
        &db_path,
        &["--json", "item", "list", "--ready", "false"],
    );
    assert_eq!(ready_false["items"].as_array().unwrap().len(), 2);

    let roots = json_output(repo.path(), &db_path, &["--json", "item", "list", "--root"]);
    assert_eq!(roots["items"].as_array().unwrap().len(), 1);
    assert_eq!(roots["items"][0]["public_id"], "WI-1");

    let by_parent = json_output(
        repo.path(),
        &db_path,
        &["--json", "item", "list", "--parent", "WI-1"],
    );
    assert_eq!(by_parent["items"].as_array().unwrap().len(), 1);
    assert_eq!(by_parent["items"][0]["public_id"], "WI-2");

    let by_priority = json_output(
        repo.path(),
        &db_path,
        &["--json", "item", "list", "--priority", "high"],
    );
    assert_eq!(by_priority["items"].as_array().unwrap().len(), 1);
    assert_eq!(by_priority["items"][0]["public_id"], "WI-1");
}

#[test]
fn item_update_status_ready_and_unready_work_and_closed_at_toggles() {
    let (repo, db_path) = setup_repo();
    json_output(repo.path(), &db_path, &["--json", "init"]);
    create_item(repo.path(), &db_path, "Task");

    let updated = json_output(
        repo.path(),
        &db_path,
        &[
            "--json",
            "item",
            "update",
            "WI-1",
            "--title",
            "Renamed",
            "--description",
            "Updated desc",
            "--priority",
            "urgent",
        ],
    );
    assert_eq!(updated["item"]["title"], "Renamed");
    assert_eq!(updated["item"]["priority"], "urgent");

    let ready = json_output(repo.path(), &db_path, &["--json", "item", "ready", "WI-1"]);
    assert_eq!(ready["item"]["ready"], true);

    let done = json_output(
        repo.path(),
        &db_path,
        &["--json", "item", "status", "WI-1", "done"],
    );
    assert_eq!(done["item"]["status"], "done");
    assert!(done["item"]["closed_at"].as_str().unwrap().ends_with('Z'));

    let reopened = json_output(
        repo.path(),
        &db_path,
        &["--json", "item", "status", "WI-1", "todo"],
    );
    assert_eq!(reopened["item"]["closed_at"], Value::Null);

    let unready = json_output(
        repo.path(),
        &db_path,
        &["--json", "item", "unready", "WI-1"],
    );
    assert_eq!(unready["item"]["ready"], false);
}

#[test]
fn move_children_and_tree_commands_render_hierarchy() {
    let (repo, db_path) = setup_repo();
    json_output(repo.path(), &db_path, &["--json", "init"]);
    create_item(repo.path(), &db_path, "Root A");
    create_item(repo.path(), &db_path, "Root B");

    let moved = json_output(
        repo.path(),
        &db_path,
        &["--json", "item", "move", "WI-2", "--parent", "WI-1"],
    );
    assert_eq!(moved["item"]["parent_id"], "WI-1");

    let children = json_output(
        repo.path(),
        &db_path,
        &["--json", "item", "children", "WI-1"],
    );
    assert_eq!(children["children"].as_array().unwrap().len(), 1);
    assert_eq!(children["children"][0]["public_id"], "WI-2");

    let tree = json_output(repo.path(), &db_path, &["--json", "item", "tree"]);
    assert_eq!(tree["tree"].as_array().unwrap().len(), 1);
    assert_eq!(tree["tree"][0]["item"]["public_id"], "WI-1");
    assert_eq!(tree["tree"][0]["children"][0]["item"]["public_id"], "WI-2");

    let subtree = json_output(repo.path(), &db_path, &["--json", "item", "tree", "WI-1"]);
    assert_eq!(subtree["tree"].as_array().unwrap().len(), 1);
    assert_eq!(subtree["tree"][0]["item"]["public_id"], "WI-1");

    let root_again = json_output(
        repo.path(),
        &db_path,
        &["--json", "item", "move", "WI-2", "--root"],
    );
    assert_eq!(root_again["item"]["parent_id"], Value::Null);
}

#[test]
fn review_tree_command_surfaces_review_state_in_json_and_human_output() {
    let (repo, db_path) = setup_repo();
    json_output(repo.path(), &db_path, &["--json", "init"]);
    create_item(repo.path(), &db_path, "Parent");
    create_item(repo.path(), &db_path, "Child");

    success_output(
        repo.path(),
        &db_path,
        &["item", "move", "WI-2", "--parent", "WI-1"],
    );
    success_output(repo.path(), &db_path, &["item", "ready", "WI-1"]);

    let review_tree = json_output(repo.path(), &db_path, &["--json", "review", "tree"]);
    assert_eq!(review_tree["tree"].as_array().unwrap().len(), 1);
    assert_eq!(review_tree["tree"][0]["item"]["public_id"], "WI-1");
    assert_eq!(review_tree["tree"][0]["review_state"], "WAIT");
    assert_eq!(review_tree["tree"][0]["has_unready_descendants"], true);
    assert_eq!(
        review_tree["tree"][0]["children"][0]["item"]["public_id"],
        "WI-2"
    );
    assert_eq!(
        review_tree["tree"][0]["children"][0]["review_state"],
        "REVIEW"
    );

    let review_human = success_output(repo.path(), &db_path, &["review", "tree"]);
    let stdout = stdout_string(&review_human);
    assert!(stdout.contains("WAIT WI-1 [todo ready=true]"));
    assert!(stdout.contains("REVIEW WI-2 [todo ready=false]"));
}

#[test]
fn parent_validation_rejects_self_parent_cycles_and_conflicting_flags() {
    let (repo, db_path) = setup_repo();
    json_output(repo.path(), &db_path, &["--json", "init"]);
    create_item(repo.path(), &db_path, "A");
    create_item(repo.path(), &db_path, "B");

    let self_parent = output(
        repo.path(),
        &db_path,
        &["item", "move", "WI-1", "--parent", "WI-1"],
    );
    assert_eq!(self_parent.status.code(), Some(1));
    assert!(stderr_string(&self_parent).contains("own parent"));

    success_output(
        repo.path(),
        &db_path,
        &["item", "move", "WI-2", "--parent", "WI-1"],
    );
    let cycle = output(
        repo.path(),
        &db_path,
        &["item", "move", "WI-1", "--parent", "WI-2"],
    );
    assert_eq!(cycle.status.code(), Some(1));
    assert!(stderr_string(&cycle).contains("create a cycle"));

    let conflicting = output(
        repo.path(),
        &db_path,
        &["item", "update", "WI-1", "--parent", "WI-2", "--root"],
    );
    assert_eq!(conflicting.status.code(), Some(2));
    assert!(stderr_string(&conflicting).contains("cannot use --parent and --root together"));
}

#[test]
fn block_unblock_blockers_and_blocked_filter_work() {
    let (repo, db_path) = setup_repo();
    json_output(repo.path(), &db_path, &["--json", "init"]);
    create_item(repo.path(), &db_path, "A");
    create_item(repo.path(), &db_path, "B");

    let blocked = json_output(
        repo.path(),
        &db_path,
        &["--json", "item", "block", "WI-2", "--by", "WI-1"],
    );
    assert_eq!(blocked["blocked"]["public_id"], "WI-2");
    assert_eq!(blocked["blocker"]["public_id"], "WI-1");
    assert_eq!(blocked["added"], true);

    let blockers = json_output(
        repo.path(),
        &db_path,
        &["--json", "item", "blockers", "WI-2"],
    );
    assert_eq!(blockers["blockers"][0], "WI-1");
    assert!(blockers["blocked_by"].as_array().unwrap().is_empty());

    let blocked_items = json_output(
        repo.path(),
        &db_path,
        &["--json", "item", "list", "--blocked", "true"],
    );
    assert_eq!(blocked_items["items"].as_array().unwrap().len(), 1);
    assert_eq!(blocked_items["items"][0]["public_id"], "WI-2");

    let unblocked = json_output(
        repo.path(),
        &db_path,
        &["--json", "item", "unblock", "WI-2", "--by", "WI-1"],
    );
    assert_eq!(unblocked["added"], false);

    let blocked_items_after = json_output(
        repo.path(),
        &db_path,
        &["--json", "item", "list", "--blocked", "true"],
    );
    assert!(blocked_items_after["items"].as_array().unwrap().is_empty());
}

#[test]
fn block_validation_rejects_self_block_and_cycles() {
    let (repo, db_path) = setup_repo();
    json_output(repo.path(), &db_path, &["--json", "init"]);
    create_item(repo.path(), &db_path, "A");
    create_item(repo.path(), &db_path, "B");
    create_item(repo.path(), &db_path, "C");

    let self_block = output(
        repo.path(),
        &db_path,
        &["item", "block", "WI-1", "--by", "WI-1"],
    );
    assert_eq!(self_block.status.code(), Some(1));
    assert!(stderr_string(&self_block).contains("cannot block itself"));

    success_output(
        repo.path(),
        &db_path,
        &["item", "block", "WI-2", "--by", "WI-1"],
    );
    success_output(
        repo.path(),
        &db_path,
        &["item", "block", "WI-3", "--by", "WI-2"],
    );
    let cycle = output(
        repo.path(),
        &db_path,
        &["item", "block", "WI-1", "--by", "WI-3"],
    );
    assert_eq!(cycle.status.code(), Some(1));
    assert!(stderr_string(&cycle).contains("create a cycle"));
}

#[test]
fn next_respects_ready_blockers_terminal_states_and_open_children() {
    let (repo, db_path) = setup_repo();
    json_output(repo.path(), &db_path, &["--json", "init"]);
    create_item(repo.path(), &db_path, "Parent");
    json_output(
        repo.path(),
        &db_path,
        &[
            "--json", "item", "create", "--title", "Child", "--parent", "WI-1",
        ],
    );
    create_item(repo.path(), &db_path, "Blocked Work");
    create_item(repo.path(), &db_path, "Blocker");
    create_item(repo.path(), &db_path, "Best Candidate");

    for id in ["WI-1", "WI-2", "WI-3", "WI-4", "WI-5"] {
        json_output(repo.path(), &db_path, &["--json", "item", "ready", id]);
    }
    json_output(
        repo.path(),
        &db_path,
        &["--json", "item", "block", "WI-3", "--by", "WI-4"],
    );

    let next = json_output(repo.path(), &db_path, &["--json", "next", "--limit", "5"]);
    let ids: Vec<_> = next["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["public_id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, vec!["WI-2", "WI-4", "WI-5"]);
    assert!(!ids.contains(&"WI-1"));
    assert!(!ids.contains(&"WI-3"));

    json_output(
        repo.path(),
        &db_path,
        &["--json", "item", "status", "WI-4", "done"],
    );
    let next_after_done = json_output(repo.path(), &db_path, &["--json", "next", "--limit", "5"]);
    let ids_after_done: Vec<_> = next_after_done["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["public_id"].as_str().unwrap())
        .collect();
    assert!(ids_after_done.contains(&"WI-3"));
}

#[test]
fn next_empty_returns_exit_code_three_and_json_error_shape() {
    let (repo, db_path) = setup_repo();
    json_output(repo.path(), &db_path, &["--json", "init"]);

    let human = output(repo.path(), &db_path, &["next"]);
    assert_eq!(human.status.code(), Some(3));
    assert!(stderr_string(&human).contains("No unblocked work items are available."));

    let json = output(repo.path(), &db_path, &["--json", "next"]);
    assert_eq!(json.status.code(), Some(3));
    let payload: Value = serde_json::from_slice(&json.stderr).unwrap();
    assert_eq!(payload["error"]["code"], "empty_result");
}

#[test]
fn history_show_command_and_list_cover_recorded_events() {
    let (repo, db_path) = setup_repo();
    json_output(repo.path(), &db_path, &["--json", "init"]);
    create_item(repo.path(), &db_path, "Task");
    json_output(repo.path(), &db_path, &["--json", "item", "ready", "WI-1"]);

    let history = json_output(
        repo.path(),
        &db_path,
        &["--json", "history", "show", "WI-1"],
    );
    let operations: Vec<_> = history["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|event| event["operation"].as_str().unwrap())
        .collect();
    assert!(operations.contains(&"create"));
    assert!(operations.contains(&"ready"));

    let command = json_output(
        repo.path(),
        &db_path,
        &["--json", "history", "command", "CMD-3"],
    );
    assert_eq!(command["command"]["public_id"], "CMD-3");
    assert_eq!(command["events"][0]["entity_type"], "work_item");

    let list = json_output(repo.path(), &db_path, &["--json", "history", "list"]);
    assert_eq!(list["commands"].as_array().unwrap().len(), 3);
    assert_eq!(list["commands"][0]["public_id"], "CMD-3");
}

#[test]
fn undo_reverts_create_update_ready_block_and_unblock_commands() {
    let (repo, db_path) = setup_repo();
    json_output(repo.path(), &db_path, &["--json", "init"]);
    create_item(repo.path(), &db_path, "A");
    create_item(repo.path(), &db_path, "B");
    json_output(
        repo.path(),
        &db_path,
        &["--json", "item", "update", "WI-1", "--title", "Renamed"],
    );
    json_output(repo.path(), &db_path, &["--json", "item", "ready", "WI-1"]);
    json_output(
        repo.path(),
        &db_path,
        &["--json", "item", "block", "WI-2", "--by", "WI-1"],
    );
    json_output(
        repo.path(),
        &db_path,
        &["--json", "item", "unblock", "WI-2", "--by", "WI-1"],
    );

    let undo_unblock = json_output(repo.path(), &db_path, &["--json", "undo", "CMD-7"]);
    assert_eq!(undo_unblock["reversed_command"], "CMD-7");
    let blockers_after_unblock_undo = json_output(
        repo.path(),
        &db_path,
        &["--json", "item", "blockers", "WI-2"],
    );
    assert_eq!(blockers_after_unblock_undo["blockers"][0], "WI-1");

    let undo_block_refusal = output(repo.path(), &db_path, &["undo", "CMD-6"]);
    assert_eq!(undo_block_refusal.status.code(), Some(1));
    assert!(stderr_string(&undo_block_refusal).contains("later changes exist"));

    json_output(repo.path(), &db_path, &["--json", "undo", "CMD-5"]);
    let after_ready_undo = json_output(repo.path(), &db_path, &["--json", "item", "show", "WI-1"]);
    assert_eq!(after_ready_undo["item"]["ready"], false);

    let undo_update_refusal = output(repo.path(), &db_path, &["undo", "CMD-4"]);
    assert_eq!(undo_update_refusal.status.code(), Some(1));
    assert!(stderr_string(&undo_update_refusal).contains("later changes exist"));
}

#[test]
fn undo_reverts_block_when_no_later_relation_change_exists() {
    let (repo, db_path) = setup_repo();
    json_output(repo.path(), &db_path, &["--json", "init"]);
    create_item(repo.path(), &db_path, "A");
    create_item(repo.path(), &db_path, "B");
    json_output(
        repo.path(),
        &db_path,
        &["--json", "item", "block", "WI-2", "--by", "WI-1"],
    );

    let undone = json_output(repo.path(), &db_path, &["--json", "undo", "CMD-4"]);
    assert_eq!(undone["reversed_command"], "CMD-4");

    let blockers = json_output(
        repo.path(),
        &db_path,
        &["--json", "item", "blockers", "WI-2"],
    );
    assert!(blockers["blockers"].as_array().unwrap().is_empty());
}

#[test]
fn undo_reverts_update_when_no_later_item_change_exists() {
    let (repo, db_path) = setup_repo();
    json_output(repo.path(), &db_path, &["--json", "init"]);
    create_item(repo.path(), &db_path, "A");
    json_output(
        repo.path(),
        &db_path,
        &["--json", "item", "update", "WI-1", "--title", "Renamed"],
    );

    let undone = json_output(repo.path(), &db_path, &["--json", "undo", "CMD-3"]);
    assert_eq!(undone["reversed_command"], "CMD-3");

    let item = json_output(repo.path(), &db_path, &["--json", "item", "show", "WI-1"]);
    assert_eq!(item["item"]["title"], "A");
}

#[test]
fn undo_reverts_create_when_no_dependent_changes_exist() {
    let (repo, db_path) = setup_repo();
    json_output(repo.path(), &db_path, &["--json", "init"]);
    create_item(repo.path(), &db_path, "A");

    let undone = json_output(repo.path(), &db_path, &["--json", "undo", "CMD-2"]);
    assert_eq!(undone["reversed_command"], "CMD-2");
    assert!(item_ids(repo.path(), &db_path).is_empty());
}

#[test]
fn undo_refuses_when_later_changes_exist() {
    let (repo, db_path) = setup_repo();
    json_output(repo.path(), &db_path, &["--json", "init"]);
    create_item(repo.path(), &db_path, "Task");
    json_output(
        repo.path(),
        &db_path,
        &["--json", "item", "update", "WI-1", "--title", "First"],
    );
    json_output(
        repo.path(),
        &db_path,
        &["--json", "item", "update", "WI-1", "--title", "Second"],
    );

    let refusal = output(repo.path(), &db_path, &["undo", "CMD-3"]);
    assert_eq!(refusal.status.code(), Some(1));
    assert!(stderr_string(&refusal).contains("later changes exist"));
}

#[test]
fn undo_project_init_is_rejected_in_mvp() {
    let (repo, db_path) = setup_repo();
    json_output(repo.path(), &db_path, &["--json", "init"]);

    let refusal = output(repo.path(), &db_path, &["undo", "CMD-1"]);
    assert_eq!(refusal.status.code(), Some(1));
    assert!(stderr_string(&refusal).contains("project creation is not supported"));
}

#[test]
fn history_and_command_ids_include_undo_commands() {
    let (repo, db_path) = setup_repo();
    json_output(repo.path(), &db_path, &["--json", "init"]);
    create_item(repo.path(), &db_path, "Task");
    json_output(repo.path(), &db_path, &["--json", "undo", "CMD-2"]);

    let commands = command_ids(repo.path(), &db_path);
    assert_eq!(commands[0], "CMD-3");

    let command = json_output(
        repo.path(),
        &db_path,
        &["--json", "history", "command", "CMD-3"],
    );
    assert_eq!(command["command"]["undone_command_id"], "CMD-2");
    assert_eq!(command["events"][0]["operation"], "undo_create");
}

#[test]
fn multi_project_database_can_filter_by_project() {
    let (repo_a, db_path) = setup_repo();
    let repo_b = tempfile::tempdir().unwrap();
    Command::new("git")
        .args(["init"])
        .current_dir(repo_b.path())
        .output()
        .unwrap();

    json_output(repo_a.path(), &db_path, &["--json", "init"]);
    create_item(repo_a.path(), &db_path, "A1");
    json_output(repo_b.path(), &db_path, &["--json", "init"]);
    create_item(repo_b.path(), &db_path, "B1");

    let projects = json_output(repo_a.path(), &db_path, &["--json", "project", "list"]);
    assert_eq!(projects["projects"].as_array().unwrap().len(), 2);

    let project_b = projects["projects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|project| project["repo_root"] == path_string(repo_b.path()))
        .unwrap()["public_id"]
        .as_str()
        .unwrap()
        .to_string();

    let only_b = json_output(
        repo_a.path(),
        &db_path,
        &["--json", "item", "list", "--project", &project_b],
    );
    assert_eq!(only_b["items"].as_array().unwrap().len(), 1);
    assert_eq!(only_b["items"][0]["title"], "B1");
}

#[test]
fn database_override_path_is_honored() {
    let (repo, db_path) = setup_repo();
    success_output(repo.path(), &db_path, &["init"]);
    assert!(fs::metadata(&db_path).is_ok());
}
