use anyhow::{Context, Result};
use serde::de::DeserializeOwned;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};

use crate::HelixError;

fn hash_bytes(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

fn object_path(helix_dir: &Path, hash: &str) -> PathBuf {
    helix_dir.join("objects").join(&hash[0..2]).join(&hash[2..])
}

/// Write raw bytes to the object store, keyed by their SHA-256 hash.
/// Objects are immutable and content-addressed: writing the same bytes twice
/// is a no-op the second time, giving automatic deduplication.
pub fn write_object(helix_dir: &Path, bytes: &[u8]) -> Result<String> {
    let hash = hash_bytes(bytes);
    let path = object_path(helix_dir, &hash);

    if !path.exists() {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&path, bytes)?;
    }

    Ok(hash)
}

/// Read raw bytes back out of the object store by hash.
pub fn read_object(helix_dir: &Path, hash: &str) -> Result<Vec<u8>> {
    let path = object_path(helix_dir, hash);
    fs::read(&path).map_err(|_| HelixError::ObjectNotFound(hash.to_string()).into())
}

/// bincode-serialize a value and store it, returning its content hash.
pub fn write_serialized<T: Serialize>(helix_dir: &Path, value: &T) -> Result<String> {
    let bytes = bincode::serialize(value).context("failed to serialize object")?;
    write_object(helix_dir, &bytes)
}

/// Read an object back and bincode-deserialize it into `T`.
pub fn read_deserialized<T: DeserializeOwned>(helix_dir: &Path, hash: &str) -> Result<T> {
    let bytes = read_object(helix_dir, hash)?;
    bincode::deserialize(&bytes).with_context(|| format!("corrupt object: {hash}"))
}
