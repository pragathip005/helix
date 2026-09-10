pub mod branch;
pub mod commit;
pub mod diff;
pub mod merge;
pub mod migrate;
pub mod snapshot;
pub mod store;
pub mod types;

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::fs;
use std::path::Path;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum HelixError {
    #[error("not a Helix repository — run `helix init` first")]
    NotInitialized,
    #[error("already initialized")]
    AlreadyInitialized,
    #[error("object not found: {0}")]
    ObjectNotFound(String),
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

/// Initialise a Helix repository in the current working directory.
pub fn init(database_url: &str) -> Result<()> {
    let helix_dir = Path::new(".helix");

    if helix_dir.exists() {
        bail!("Already a Helix repository (found .helix/ in current directory)");
    }

    // Directory structure
    fs::create_dir(helix_dir)?;
    fs::create_dir(helix_dir.join("objects"))?;
    fs::create_dir_all(helix_dir.join("refs").join("heads"))?;

    // HEAD — points to main branch by default
    fs::write(helix_dir.join("HEAD"), "ref: refs/heads/main\n")?;

    // config.toml
    let config = format!(
        "[core]\nversion = 1\n\n[database]\nurl = \"{}\"\n",
        database_url
    );
    fs::write(helix_dir.join("config.toml"), config)?;

    tracing::info!("initialized Helix repository");
    println!("Initialized Helix repository.");
    Ok(())
}

#[derive(Debug, Deserialize)]
struct ConfigFile {
    database: DatabaseConfig,
}

#[derive(Debug, Deserialize)]
struct DatabaseConfig {
    url: String,
}

/// Read the database URL out of `.helix/config.toml`.
pub fn database_url() -> Result<String> {
    let helix_dir = Path::new(".helix");
    if !helix_dir.exists() {
        bail!("not a Helix repository — run `helix init` first");
    }

    let contents = fs::read_to_string(helix_dir.join("config.toml"))
        .context("failed to read .helix/config.toml")?;
    let config: ConfigFile = toml::from_str(&contents).context("failed to parse .helix/config.toml")?;
    Ok(config.database.url)
}
