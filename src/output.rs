use anyhow::Context;
use serde_json::{Value, json};

use crate::domain::{ProjectRecord, TreeNode, WorkItemRecord};
use crate::error::CliResult;
use crate::repo::ProjectResolution;

pub fn emit_project(json_output: bool, label: &str, project: &ProjectRecord) -> CliResult<()> {
    if json_output {
        emit_value(true, &json!({ "project": project }))
    } else {
        println!("{}: {} {}", label, project.public_id, project.name);
        println!("item_prefix={}", project.item_prefix);
        if let Some(repo_root) = &project.repo_root {
            println!("repo_root={repo_root}");
        }
        Ok(())
    }
}

pub fn emit_project_resolution(
    json_output: bool,
    label: &str,
    resolution: &ProjectResolution,
) -> CliResult<()> {
    if json_output {
        emit_value(
            true,
            &json!({ "project": resolution.project, "resolution": {
                "source": resolution.source,
                "repo_root": resolution.repo_root,
                "override_project_id": resolution.override_project_id,
                "created": resolution.created,
            }}),
        )
    } else {
        emit_project(false, label, &resolution.project)?;
        println!("resolved_by={}", resolution.source);
        println!("created={}", resolution.created);
        if let Some(project_id) = &resolution.override_project_id {
            println!("override_project_id={project_id}");
        }
        if let Some(repo_root) = &resolution.repo_root {
            println!("resolution_repo_root={repo_root}");
        }
        Ok(())
    }
}

pub fn emit_item(json_output: bool, label: &str, item: &WorkItemRecord) -> CliResult<()> {
    if json_output {
        emit_value(true, &json!({ "item": item }))
    } else {
        println!("{}: {} {}", label, item.public_id, item.title);
        println!(
            "status={} priority={} ready={}",
            item.status, item.priority, item.ready
        );
        Ok(())
    }
}

pub fn emit_item_for_project(
    json_output: bool,
    label: &str,
    project: &ProjectRecord,
    item: &WorkItemRecord,
) -> CliResult<()> {
    if json_output {
        emit_value(true, &json!({ "project": project, "item": item }))
    } else {
        emit_item(false, label, item)?;
        println!("project={}", project.public_id);
        Ok(())
    }
}

pub fn emit_value(json_output: bool, value: &Value) -> CliResult<()> {
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(value).context("failed to render JSON output")?
        );
    } else if let Some(item) = value.get("item") {
        println!(
            "{}",
            serde_json::to_string_pretty(item).context("failed to render value")?
        );
    } else {
        println!(
            "{}",
            serde_json::to_string_pretty(value).context("failed to render value")?
        );
    }
    Ok(())
}

pub fn render_tree(node: &TreeNode, depth: usize) {
    let indent = "  ".repeat(depth);
    println!(
        "{}{} [{} ready={}] {}",
        indent, node.item.public_id, node.item.status, node.item.ready, node.item.title
    );
    for child in &node.children {
        render_tree(child, depth + 1);
    }
}
