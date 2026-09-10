# Helix

Git-style version control for PostgreSQL database schemas.

Helix points at a live Postgres database, extracts its schema, and stores it as an
immutable, content-addressed snapshot — the same model Git uses for files. From there you
get commit history, branches, three-way merges, human-readable diffs, and Flyway-compatible
migration export, all without Helix ever writing to your database.

See [KICKSTART.md](KICKSTART.md) for the full spec, architecture, setup instructions, and
project context.

## Status

Work in progress. Implemented so far:

- **Story 1.1** — `helix init <database-url>` scaffolds a `.helix/` repository
  (`config.toml`, `HEAD`, `objects/`, `refs/heads/`)

## Quick start

```bash
cargo build
cargo run --bin helix -- init postgres://helix:helix@localhost:5433/helix_dev
```

See [KICKSTART.md](KICKSTART.md) §3–§7 for full environment setup (Rust, Docker, Postgres).
