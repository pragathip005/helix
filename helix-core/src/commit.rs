use anyhow::{bail, Context, Result};
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::store;
use crate::types::{Commit, Schema};

fn helix_dir() -> PathBuf {
    PathBuf::from(".helix")
}

fn require_repo() -> Result<PathBuf> {
    let dir = helix_dir();
    if !dir.exists() {
        bail!("not a Helix repository — run `helix init` first");
    }
    Ok(dir)
}

/// The branch HEAD currently points at (e.g. "main").
pub fn current_branch() -> Result<String> {
    let dir = require_repo()?;
    let head = fs::read_to_string(dir.join("HEAD")).context("failed to read HEAD")?;
    head.trim()
        .strip_prefix("ref: refs/heads/")
        .map(str::to_string)
        .context("HEAD is not pointing at a branch")
}

pub fn branch_ref_path(branch: &str) -> PathBuf {
    helix_dir().join("refs").join("heads").join(branch)
}

/// The commit hash the current branch points at, or `None` if the branch has no commits yet.
pub fn head_commit_hash() -> Result<Option<String>> {
    let branch = current_branch()?;
    let ref_path = branch_ref_path(&branch);
    if !ref_path.exists() {
        return Ok(None);
    }
    let hash = fs::read_to_string(&ref_path)?.trim().to_string();
    Ok(if hash.is_empty() { None } else { Some(hash) })
}

pub fn load_commit(hash: &str) -> Result<Commit> {
    store::read_deserialized(&helix_dir(), hash)
}

pub fn load_schema(hash: &str) -> Result<Schema> {
    store::read_deserialized(&helix_dir(), hash)
}

/// Snapshot `schema` as a new commit, becoming the new tip of the current branch.
pub fn create_commit(schema: &Schema, message: &str) -> Result<String> {
    let dir = require_repo()?;

    let schema_hash = store::write_serialized(&dir, schema)?;
    let parent_hash = head_commit_hash()?;
    let timestamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() as i64;

    let commit = Commit {
        parent_hash,
        schema_hash,
        timestamp,
        message: message.to_string(),
    };
    let commit_hash = store::write_serialized(&dir, &commit)?;

    let branch = current_branch()?;
    fs::write(branch_ref_path(&branch), format!("{commit_hash}\n"))?;
    tracing::info!(commit = %commit_hash, branch, "created commit");

    Ok(commit_hash)
}

/// The commit hash a named branch currently points at.
///
/// Validates `branch` before it ever touches the filesystem — this is the one safe
/// entry point for turning a CLI-supplied branch name into a path read, used by every
/// caller (including `resolve_schema` below) instead of joining the raw name in directly.
pub fn branch_head_hash(branch: &str) -> Result<String> {
    crate::branch::validate_branch_name(branch)?;
    let path = branch_ref_path(branch);
    let contents = fs::read_to_string(&path).map_err(|_| anyhow::anyhow!("branch '{branch}' does not exist"))?;
    Ok(contents.trim().to_string())
}

/// Resolve a branch name or commit hash to the `Schema` it points at.
pub fn resolve_schema(reference: &str) -> Result<Schema> {
    let commit_hash = branch_head_hash(reference).unwrap_or_else(|_| reference.to_string());
    let commit = load_commit(&commit_hash)
        .with_context(|| format!("'{reference}' is not a known branch or commit"))?;
    load_schema(&commit.schema_hash)
}

/// Walk the commit DAG from HEAD back to the root, following parent pointers.
/// Returns commits newest-first.
pub fn log() -> Result<Vec<(String, Commit)>> {
    let mut history = Vec::new();
    let mut current = head_commit_hash()?;

    while let Some(hash) = current {
        let commit = load_commit(&hash)?;
        current = commit.parent_hash.clone();
        history.push((hash, commit));
    }

    Ok(history)
}
