use anyhow::{bail, Context, Result};
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

/// A hash used to address an object must be exactly 64 lowercase hex characters (a
/// SHA-256 digest) before it is ever used to build a filesystem path. Object hashes
/// reach this code from CLI arguments (a commit or branch reference typed by the user)
/// as well as from our own internal callers, so this is a security boundary, not just
/// a sanity check: without it, a crafted value like `../../../../etc/shadow` would
/// slice into a path that escapes `.helix/objects/` entirely (`Path::join` treats a
/// leading '/' as replacing everything before it), and a value shorter than 2 bytes
/// would panic on the slice below instead of failing cleanly.
fn validate_hash(hash: &str) -> Result<()> {
    let is_valid = hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit());
    if !is_valid {
        tracing::warn!(hash, "rejected malformed object hash");
        bail!("'{hash}' is not a valid object hash");
    }
    Ok(())
}

fn object_path(helix_dir: &Path, hash: &str) -> Result<PathBuf> {
    validate_hash(hash)?;
    Ok(helix_dir.join("objects").join(&hash[0..2]).join(&hash[2..]))
}

/// Write raw bytes to the object store, keyed by their SHA-256 hash.
/// Objects are immutable and content-addressed: writing the same bytes twice
/// is a no-op the second time, giving automatic deduplication.
pub fn write_object(helix_dir: &Path, bytes: &[u8]) -> Result<String> {
    let hash = hash_bytes(bytes);
    let path = object_path(helix_dir, &hash)?;

    if path.exists() {
        tracing::debug!(hash, "object already present, skipping write (dedup)");
    } else {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create object directory for {hash}"))?;
        }
        fs::write(&path, bytes).with_context(|| format!("failed to write object {hash}"))?;
        tracing::debug!(hash, bytes = bytes.len(), "wrote object");
    }

    Ok(hash)
}

/// Read raw bytes back out of the object store by hash.
pub fn read_object(helix_dir: &Path, hash: &str) -> Result<Vec<u8>> {
    let path = object_path(helix_dir, hash)?;
    fs::read(&path).map_err(|err| {
        tracing::error!(hash, error = %err, "object not found");
        HelixError::ObjectNotFound(hash.to_string()).into()
    })
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_path_traversal_disguised_as_a_hash() {
        let dir = std::env::temp_dir().join(format!("helix-store-test-{}", std::process::id()));
        let result = read_object(&dir, "../../../../../../etc/shadow");
        assert!(result.is_err());
    }

    #[test]
    fn rejects_short_hash_without_panicking() {
        let dir = std::env::temp_dir().join(format!("helix-store-test-{}", std::process::id()));
        let result = read_object(&dir, "a");
        assert!(result.is_err());
    }

    #[test]
    fn accepts_well_formed_hash_shape() {
        assert!(validate_hash(&"a".repeat(64)).is_ok());
    }
}
