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
    let branch_path = heads_dir().join(name);
    if branch_path.exists() {
        bail!("branch '{name}' already exists");
    }

    let head_hash = commit::head_commit_hash()?
        .context("cannot branch — no commits yet, run `helix commit` first")?;

    fs::write(branch_path, format!("{head_hash}\n"))?;
    Ok(())
}

/// Point HEAD at the given branch.
pub fn checkout(name: &str) -> Result<()> {
    if !heads_dir().join(name).exists() {
        bail!("branch '{name}' does not exist");
    }

    fs::write(helix_dir().join("HEAD"), format!("ref: refs/heads/{name}\n"))?;
    Ok(())
}
