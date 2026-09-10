use anyhow::{bail, Context, Result};
use std::fs;
use std::path::PathBuf;

use crate::commit;

fn helix_dir() -> PathBuf {
    PathBuf::from(".helix")
}

fn heads_dir() -> PathBuf {
    helix_dir().join("refs").join("heads")
}

/// A branch name must be safe to use as a single filesystem path segment: only ASCII
/// letters, digits, `-`, `_`, and `.`, never empty, and never starting with `.`
/// (which would rule out both hidden files and `..`). Branch names reach this code
/// straight from CLI arguments, so this is what stops `helix branch '../../etc/cron.d/x'`
/// (or similar) from writing a ref file outside `.helix/refs/heads/`.
pub(crate) fn validate_branch_name(name: &str) -> Result<()> {
    let is_safe = !name.is_empty()
        && !name.starts_with('.')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.');

    if !is_safe {
        tracing::warn!(name, "rejected unsafe branch name");
        bail!("'{name}' is not a valid branch name (use letters, digits, '-', '_', '.')");
    }
    Ok(())
}

/// List all branches and the commit hash each currently points at.
pub fn list() -> Result<Vec<(String, String)>> {
    let dir = heads_dir();
    let mut branches = Vec::new();

    for entry in fs::read_dir(&dir).context("not a Helix repository — run `helix init` first")? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        let hash = fs::read_to_string(entry.path())?.trim().to_string();
        branches.push((name, hash));
    }

    branches.sort();
    Ok(branches)
}

/// Create a new branch pointing at the current HEAD commit.
pub fn create(name: &str) -> Result<()> {
    validate_branch_name(name)?;

    let branch_path = heads_dir().join(name);
    if branch_path.exists() {
        bail!("branch '{name}' already exists");
    }

    let head_hash = commit::head_commit_hash()?
        .context("cannot branch — no commits yet, run `helix commit` first")?;

    fs::write(branch_path, format!("{head_hash}\n"))?;
    tracing::info!(branch = name, commit = %head_hash, "created branch");
    Ok(())
}

/// Point HEAD at the given branch.
pub fn checkout(name: &str) -> Result<()> {
    validate_branch_name(name)?;

    if !heads_dir().join(name).exists() {
        bail!("branch '{name}' does not exist");
    }

    fs::write(helix_dir().join("HEAD"), format!("ref: refs/heads/{name}\n"))?;
    tracing::info!(branch = name, "switched HEAD");
    Ok(())
}
