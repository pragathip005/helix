# Helix — Project Kickstart

> This document lives in the root of the repo. It is the single source of truth for
> getting the project running from zero, understanding what you built well enough to
> defend it in an interview, and knowing where every piece of code belongs.
>
> Read it top to bottom before writing any code.  
> If something is wrong or out of date, fix it and commit.

---

## Table of Contents

1. [What You're Building](#1-what-youre-building)
2. [The Mental Model](#2-the-mental-model)
3. [Prerequisites — Linux Mint Setup](#3-prerequisites--linux-mint-setup)
4. [Repo Structure](#4-repo-structure)
5. [Setting Up the Workspace](#5-setting-up-the-workspace)
6. [Setting Up the Database](#6-setting-up-the-database)
7. [Environment Variables](#7-environment-variables)
8. [The .helix/ Directory](#8-the-helix-directory)
9. [Crate Responsibilities](#9-crate-responsibilities)
10. [Core Data Structs — Define These First](#10-core-data-structs--define-these-first)
11. [Story 1.1 — Your First Task](#11-story-11--your-first-task)
12. [Running and Testing](#12-running-and-testing)
13. [Git Workflow for This Project](#13-git-workflow-for-this-project)
14. [Common Errors and Fixes](#14-common-errors-and-fixes)
15. [Rust Concepts You'll Hit in Week 1](#15-rust-concepts-youll-hit-in-week-1)
16. [How to Explain This Project](#16-how-to-explain-this-project)
17. [Interview Attack Sequence](#17-interview-attack-sequence)
18. [Decision Log](#18-decision-log)

---

## 1. What You're Building

Helix is a CLI tool that brings Git-style version control to Postgres database schemas.

**The core loop:**  
You point Helix at a live Postgres database. It extracts the schema — all tables, columns,
types, constraints, indexes — and stores a serialized snapshot in a content-addressed object
store at `.helix/`. Each snapshot is a commit with a parent pointer, timestamp, and message,
exactly like Git. From that you can diff any two commits, create branches, and merge schema
changes across branches. The diff engine works on parsed SQL ASTs, not raw strings, which
is what makes it semantically correct.

**What it is NOT:**
- It does not track row data — schema only
- It does not write to your database — read-only against Postgres
- It is not a migration runner (that's Flyway's job) — it figures out *what the migration should be*

**The four differentiators vs Atlas (the main competitor):**
1. Zero-config — just a DB URL, no HCL schema files to maintain
2. True commit history — snapshot at every commit, diff any two points in time
3. Human-readable diffs — `↷ email → email_address` not `DROP COLUMN + ADD COLUMN`
4. Rename detection — heuristic that catches renames instead of showing false data loss

---

## 2. The Mental Model

Read this section carefully. This is what you say when someone asks "explain your project."

### Git does this to code files. Helix does the same to database schemas.

```
Git:                              Helix:

Version 1 of code                 PostgreSQL Schema V1
       ↓                                 ↓
    commit                            commit
       ↓                                 ↓
Version 2                          Schema V2
       ↓                                 ↓
    branch                           branch
      / \                              / \
     /   \                            /   \
  V3A    V3B                      Schema A  Schema B
      \ /                              \ /
     merge                            merge
```

Instead of tracking files like `main.py`, `server.js`, Helix tracks:

```
users table
orders table
products table
columns, constraints, indexes, foreign keys
```

### The five major pieces

```
                    HELIX
                      │
      ┌───────────────┼────────────────┐
      │               │                │
      ▼               ▼                ▼
 Object Store     Commit DAG        Branches
      │               │                │
      └───────────────┼────────────────┘
                      │
                      ▼
                 Merge Engine
                      │
                      ▼
                 Schema Diff
                      │
                      ▼
                  Migration
```

### The full pipeline from schema to output

```
PostgreSQL schema
       ↓
information_schema queries (REPEATABLE READ transaction)
       ↓
Schema struct (Table → Column → Constraint → Index)
       ↓
bincode serialization → bytes
       ↓
SHA-256 hash → object identity
       ↓
stored as immutable object in .helix/objects/
       ↓
Commit { parent_hash, schema_hash, timestamp, message }
       ↓
DAG of commits (parent pointers)
       ↓
branches (named pointers to commit hashes)
       ↓  ↓
     diff  merge
       ↓
DiffOp list (AddTable, DropColumn, RenameColumn, ...)
       ↓
human-readable colored output   OR   Flyway-compatible SQL migration
```

That pipeline is the whole project. Every module in the codebase corresponds to one
stage of that pipeline.

### Why content-addressed storage?

Normal storage: `object ID → 12345`  
Content-addressed: `hash(content) → identifier`

```
Schema Object
     ↓
SHA-256
     ↓
4f2a8bc9...
```

**Key properties this gives you:**

1. Same content = same hash = same file = automatic deduplication. If the schema hasn't
   changed between two commits, the same schema object is shared — nothing is duplicated.

2. Objects are immutable. You never modify an existing object — you create a new one.
   Old history stays intact permanently.

3. Copy-on-write branching is free. A branch is just a text file containing a commit
   hash. Creating a branch costs zero disk space — both branches share all existing
   objects until something actually changes on one of them.

### Why not string-diff the SQL?

Three concrete reasons:

1. **Rename looks like data loss.** `email` → `email_address` appears as DROP + ADD in
   a text diff. That's a false data loss warning. Helix's AST diff detects it as a rename.

2. **Column ordering produces false diffs.** Column order in SQL has no semantic meaning
   but text diff treats it as a change. AST diff compares fields, not lines.

3. **Whitespace and equivalent syntax vary.** `NOT  NULL` vs `NOT NULL` — different text,
   same meaning. AST comparison is whitespace-agnostic.

### The merge truth table

This is the core logic behind `helix merge`. Memorize it.

| Base | Ours | Theirs | Result     | Reason                          |
|------|------|--------|------------|---------------------------------|
| A    | A    | A      | A          | Nobody changed it               |
| A    | B    | A      | B          | Only ours changed — take ours   |
| A    | A    | B      | B          | Only theirs changed — take it   |
| A    | B    | B      | B          | Both made same change — no conflict |
| A    | B    | C      | **CONFLICT** | Both changed differently      |

This applies at the schema node level — per column, per table, per constraint. Two branches
modifying `users.email` in different ways → conflict. Two branches adding different columns
to different tables → auto-merge both.

### Merge edge cases to know cold

**Drop vs Modify conflict:**
```
Base:   users table exists
Ours:   DROP users
Theirs: ALTER users ADD COLUMN email
Result: CONFLICT — can't apply a column change to a dropped table
```

**Add-Add same column, same definition:**
```
Base:   users(id)
Ours:   users(id, email VARCHAR(100))
Theirs: users(id, email VARCHAR(100))
Result: AUTO-MERGE — both made identical change
```

**Add-Add same column, different definition:**
```
Base:   users(id)
Ours:   users(id, email VARCHAR(100))
Theirs: users(id, email VARCHAR(200))
Result: CONFLICT — same column name, different types
```

**Rename vs Rename conflict:**
```
Base:   users.username
Ours:   users.user_name
Theirs: users.login_name
Result: CONFLICT — incompatible renames of same column
```

**Why three-way, not two-way?**  
If you only compare Ours vs Theirs without the base, you can't tell whether a value was
"changed by both branches independently" or "was always different." The base is the
reference point that makes conflict detection possible.

---

## 3. Prerequisites — Linux Mint Setup

You're on Linux Mint (Debian/Ubuntu-based). Every command below is specific to that.
Run them in order. Verify each one before moving on.

### 3.1 System packages

```bash
sudo apt-get update
sudo apt-get install -y \
    curl \
    git \
    build-essential \
    pkg-config \
    libssl-dev \
    postgresql-client
```

`build-essential` gives you `gcc` and `make` — required for compiling some Rust crates
that link to C libraries. `libssl-dev` is needed by sqlx for TLS connections.
`postgresql-client` gives you `psql` to inspect your database manually.

Verify:

```bash
git --version          # 2.x+
psql --version         # 14+ is fine
gcc --version          # any version
```

### 3.2 Rust

Install via `rustup` — not `apt`. The `apt` version of Rust is always outdated.

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

When prompted, choose option 1 (default installation). After it finishes:

```bash
source $HOME/.cargo/env
```

Add that line to your `~/.bashrc` so it persists across terminal sessions:

```bash
echo 'source $HOME/.cargo/env' >> ~/.bashrc
```

Verify:

```bash
rustc --version    # should be 1.75.0 or newer
cargo --version    # same version
```

### 3.3 Docker

Linux Mint requires Docker Engine, not Docker Desktop. The `docker.io` package from
apt is outdated and broken — use the official Docker repo.

```bash
# Remove any old versions first
sudo apt-get remove docker docker-engine docker.io containerd runc 2>/dev/null

# Add Docker's official GPG key
sudo apt-get install -y ca-certificates curl gnupg
sudo install -m 0755 -d /etc/apt/keyrings
curl -fsSL https://download.docker.com/linux/ubuntu/gpg | \
    sudo gpg --dearmor -o /etc/apt/keyrings/docker.gpg
sudo chmod a+r /etc/apt/keyrings/docker.gpg

# Add Docker repo (Linux Mint is based on Ubuntu — use the Ubuntu codename)
# First check your Ubuntu base:
. /etc/upstream-release/lsb-release 2>/dev/null || . /etc/os-release
echo $UBUNTU_CODENAME   # should print something like "jammy" or "focal"

# Add the repo using that codename (replace jammy if yours differs)
echo "deb [arch=$(dpkg --print-architecture) signed-by=/etc/apt/keyrings/docker.gpg] \
https://download.docker.com/linux/ubuntu jammy stable" | \
sudo tee /etc/apt/sources.list.d/docker.list > /dev/null

# Install
sudo apt-get update
sudo apt-get install -y docker-ce docker-ce-cli containerd.io docker-compose-plugin
```

Add yourself to the docker group so you don't need `sudo` every time:

```bash
sudo usermod -aG docker $USER
newgrp docker    # applies immediately in this session
```

Verify:

```bash
docker --version           # Docker version 24.x+
docker compose version     # Docker Compose version 2.x
docker run hello-world     # should pull and print "Hello from Docker!"
```

### 3.4 VS Code + rust-analyzer

If you don't have VS Code:

```bash
# Download the .deb from https://code.visualstudio.com/
# Then install it:
sudo dpkg -i ~/Downloads/code_*.deb
```

Inside VS Code, install the **rust-analyzer** extension (Extension ID: `rust-lang.rust-analyzer`).
This is not optional. It gives you inline type hints, error squiggles as you type, and
autocomplete for Rust. Without it, fighting the type system as a beginner takes 3× longer.

Also install: **Even Better TOML** (for Cargo.toml syntax highlighting).

---

## 4. Repo Structure

Create this layout exactly. Do not add extra files or directories in Week 1.

```
helix/
├── Cargo.toml              ← workspace root (lists all member crates)
├── Cargo.lock              ← commit this — it's a binary project, not a library
├── KICKSTART.md            ← this file
├── README.md               ← write this in Week 8, not now
├── .gitignore
├── .env.example            ← commit this — shows required vars without values
├── .env                    ← DO NOT commit — your local values
├── docker-compose.yml      ← spins up dev + test Postgres
│
├── .github/
│   └── workflows/
│       └── ci.yml          ← GitHub Actions: build + test on every PR
│
├── helix-cli/              ← binary crate — CLI entrypoint only
│   ├── Cargo.toml
│   └── src/
│       └── main.rs         ← argument parsing + calls into helix-core
│
├── helix-core/             ← library crate — all business logic
│   ├── Cargo.toml
│   └── src/
│       ├── lib.rs          ← public API + module declarations
│       ├── types.rs        ← Schema, Table, Column, Commit, DiffOp — agree on this first
│       ├── snapshot.rs     ← Postgres connection + schema extraction
│       ├── store.rs        ← .helix/ content-addressed object store
│       ├── commit.rs       ← Commit struct, HEAD pointer, refs/heads/ management
│       ├── diff.rs         ← AST parsing + DiffOp generation + rename detection
│       ├── branch.rs       ← branch create, list, checkout
│       ├── merge.rs        ← LCA algorithm + 3-way merge + conflict detection
│       └── migrate.rs      ← DiffOps → SQL migration export
│
└── tests/
    ├── fixtures/
    │   └── seed.sql        ← realistic schema for dev + integration tests
    └── integration/
        └── basic.rs        ← integration tests that hit Docker Postgres
```

**Why two crates?**  
`helix-cli` handles argument parsing and calls into `helix-core`. `helix-core` contains
all logic and is testable without the CLI. This means you can write unit tests for the
diff engine without invoking the binary, and it means helix-core could eventually be
used as a library by someone else's tooling.

---

## 5. Setting Up the Workspace

Do this in order. Don't skip steps.

### Step 1 — Create the workspace

```bash
mkdir helix && cd helix
git init
```

### Step 2 — Create the workspace Cargo.toml

Create `Cargo.toml` in the project root (not inside any crate):

```toml
[workspace]
members = [
    "helix-cli",
    "helix-core",
]
resolver = "2"
```

`resolver = "2"` enables Cargo's newer feature resolver. Always use it for new projects.

### Step 3 — Create the two crates

```bash
cargo new helix-cli --bin    # creates helix-cli/src/main.rs
cargo new helix-core --lib   # creates helix-core/src/lib.rs
```

### Step 4 — Set up dependencies

**helix-cli/Cargo.toml** — replace `[dependencies]`:

```toml
[dependencies]
helix-core = { path = "../helix-core" }
clap       = { version = "4", features = ["derive"] }
tokio      = { version = "1", features = ["full"] }
anyhow     = "1"
dotenvy    = "0.15"
```

**helix-core/Cargo.toml** — replace `[dependencies]`:

```toml
[dependencies]
# Database
sqlx = { version = "0.7", features = ["runtime-tokio-rustls", "postgres", "macros"] }
tokio = { version = "1", features = ["full"] }

# SQL parsing
sqlparser = "0.43"

# Serialization
serde    = { version = "1", features = ["derive"] }
bincode  = "1"

# Hashing
sha2 = "0.10"
hex  = "0.4"

# Error handling
thiserror = "1"
anyhow    = "1"

# Config
toml = "0.8"

# Terminal output
colored = "2"

[dev-dependencies]
tokio = { version = "1", features = ["full", "test-util"] }
```

### Step 5 — Create stub source files

Every module declared in `lib.rs` needs a file to exist or the build fails.

```bash
touch helix-core/src/types.rs
touch helix-core/src/snapshot.rs
touch helix-core/src/store.rs
touch helix-core/src/commit.rs
touch helix-core/src/diff.rs
touch helix-core/src/branch.rs
touch helix-core/src/merge.rs
touch helix-core/src/migrate.rs
```

### Step 6 — Verify it builds

```bash
cargo build
```

First run downloads all dependencies — takes 2–4 minutes on a fresh machine. If it prints
`Finished dev` at the end with no errors, you're set.

### Step 7 — Create .gitignore

```
/target
.env
*.orig
```

### Step 8 — First commit

```bash
git add .
git commit -m "chore: cargo workspace scaffold with all dependencies"
```

---

## 6. Setting Up the Database

### docker-compose.yml

Create this in the project root:

```yaml
version: '3.8'

services:
  postgres:
    image: postgres:16-alpine
    environment:
      POSTGRES_USER: helix
      POSTGRES_PASSWORD: helix
      POSTGRES_DB: helix_dev
    ports:
      - "5433:5432"
    volumes:
      - helix_pg_data:/var/lib/postgresql/data
      - ./tests/fixtures/seed.sql:/docker-entrypoint-initdb.d/seed.sql

  postgres_test:
    image: postgres:16-alpine
    environment:
      POSTGRES_USER: helix_test
      POSTGRES_PASSWORD: helix_test
      POSTGRES_DB: helix_test
    ports:
      - "5434:5432"
    tmpfs:
      - /var/lib/postgresql/data

volumes:
  helix_pg_data:
```

Two databases:
- `helix_dev` on port **5433** — persistent dev database, pre-seeded with a realistic schema
- `helix_test` on port **5434** — in-memory test database, wiped on every container restart

Port 5433 avoids conflicts if you have a local Postgres installation on 5432.

### tests/fixtures/seed.sql

```bash
mkdir -p tests/fixtures
```

Create `tests/fixtures/seed.sql`:

```sql
-- Realistic e-commerce schema — used for development and integration tests

CREATE TABLE users (
    id         SERIAL PRIMARY KEY,
    email      VARCHAR(255) NOT NULL UNIQUE,
    name       VARCHAR(100) NOT NULL,
    created_at TIMESTAMP DEFAULT NOW(),
    updated_at TIMESTAMP DEFAULT NOW()
);

CREATE TABLE products (
    id         SERIAL PRIMARY KEY,
    name       VARCHAR(200) NOT NULL,
    price      DECIMAL(10,2) NOT NULL,
    stock      INTEGER DEFAULT 0,
    created_at TIMESTAMP DEFAULT NOW()
);

CREATE TABLE orders (
    id         SERIAL PRIMARY KEY,
    user_id    INTEGER NOT NULL REFERENCES users(id),
    total      DECIMAL(10,2) NOT NULL,
    status     VARCHAR(20) DEFAULT 'pending',
    created_at TIMESTAMP DEFAULT NOW()
);

CREATE TABLE order_items (
    id         SERIAL PRIMARY KEY,
    order_id   INTEGER NOT NULL REFERENCES orders(id),
    product_id INTEGER NOT NULL REFERENCES products(id),
    quantity   INTEGER NOT NULL,
    price      DECIMAL(10,2) NOT NULL
);

CREATE INDEX idx_orders_user_id       ON orders(user_id);
CREATE INDEX idx_order_items_order_id ON order_items(order_id);
```

### Start the databases

```bash
docker compose up -d

# Verify both containers are running
docker compose ps

# Connect to dev DB and confirm seed worked
psql postgres://helix:helix@localhost:5433/helix_dev -c "\dt"
# Expected output: users, products, orders, order_items
```

### Useful docker commands

```bash
docker compose up -d              # start in background
docker compose down               # stop (keeps data)
docker compose down -v            # stop AND wipe all volumes (re-runs seed.sql)
docker compose logs postgres      # show Postgres logs
docker compose ps                 # show running containers and their status
```

---

## 7. Environment Variables

### .env.example — commit this

```bash
# Development database (Docker)
DATABASE_URL=postgres://helix:helix@localhost:5433/helix_dev

# Test database (Docker — in-memory, wiped on restart)
TEST_DATABASE_URL=postgres://helix_test:helix_test@localhost:5434/helix_test
```

### .env — DO NOT commit (it's in .gitignore)

```bash
cp .env.example .env
# The values in .env.example already match docker-compose.yml — no edits needed
```

### Loading .env in the CLI

In `helix-cli/src/main.rs`, at the top of `main()`:

```rust
// Load .env if it exists — for local development convenience
dotenvy::dotenv().ok();   // .ok() silently ignores missing file
```

In test files that need `TEST_DATABASE_URL`:

```rust
dotenvy::dotenv().ok();
let db_url = std::env::var("TEST_DATABASE_URL")
    .expect("TEST_DATABASE_URL must be set for integration tests");
```

---

## 8. The .helix/ Directory

This is what Helix creates when you run `helix init`. Understanding this layout is
understanding the entire storage engine.

```
.helix/
├── config.toml
├── HEAD
├── objects/
│   ├── 4f/
│   │   └── 2a8bc9d3e1f7a0b5c8d2e9f3a6b1c4d7e0f2a5b8
│   └── 9e/
│       └── 1d3a1f7b2c4d5e6f0a1b2c3d4e5f6a7b8c9d0e1f
└── refs/
    └── heads/
        ├── main
        └── feature-payments
```

**config.toml:**
```toml
[core]
version = 1

[database]
url = "postgres://helix:helix@localhost:5433/helix_dev"
```

**HEAD:**
```
ref: refs/heads/main
```

**refs/heads/main:**
```
9e1d3a1f7b2c4d5e6f0a1b2c3d4e5f6a7b8c9d0e1f
```

**objects/4f/2a8bc9...:**  
Binary blob — bincode-serialized Schema or Commit struct, stored by its SHA-256 hash.

Every Helix operation is either:
- A read from `.helix/` + a Postgres query, or
- A write to `.helix/`

Helix **never writes to your database**.

---

## 9. Crate Responsibilities

Clear ownership prevents merge conflicts. Respect this split.

| File | Owner | What it does |
|---|---|---|
| `helix-cli/src/main.rs` | Blitz | CLI parsing with clap, calls into helix-core |
| `helix-core/src/types.rs` | **Both — agree first** | Schema, Table, Column, Commit, DiffOp |
| `helix-core/src/snapshot.rs` | Pragathi | Postgres connection, catalog queries, Schema extraction |
| `helix-core/src/store.rs` | Blitz | Object store: write/read by SHA-256 hash |
| `helix-core/src/commit.rs` | Blitz | Commit struct, HEAD pointer, refs management |
| `helix-core/src/diff.rs` | Pragathi | sqlparser-rs AST parsing, DiffOp generation, rename detection |
| `helix-core/src/branch.rs` | Blitz | Branch create, list, checkout |
| `helix-core/src/merge.rs` | Pragathi | LCA, 3-way merge, conflict detection |
| `helix-core/src/migrate.rs` | Blitz | DiffOps → SQL generation, FK ordering |

**The one rule that matters:** `types.rs` is shared infrastructure. Every other module
depends on it. Before adding or changing a field on `Schema`, `Table`, `Column`, or
`DiffOp` — talk to each other first. A field you add to `Column` in Week 2 that Pragathi's
diff engine doesn't expect will cost both of you a day in Week 4.

---

## 10. Core Data Structs — Define These First

Before writing any logic, both of you agree on `helix-core/src/types.rs`. This file is
the contract between all modules. Get it right before anyone writes `snapshot.rs` or
`diff.rs`.

```rust
// helix-core/src/types.rs

use serde::{Deserialize, Serialize};

/// Full schema of a database at a point in time.
/// This is what gets snapshotted and stored on every `helix commit`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Schema {
    pub tables: Vec<Table>,
}

/// A single table in the schema.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Table {
    pub name: String,
    pub columns: Vec<Column>,
    pub constraints: Vec<Constraint>,
    pub indexes: Vec<Index>,
}

/// A single column within a table.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Column {
    pub name: String,
    pub ordinal_position: i32,       // 1-based — CRITICAL for rename detection
    pub data_type: String,           // e.g. "character varying", "integer"
    pub char_max_length: Option<i32>, // e.g. 255 for VARCHAR(255)
    pub is_nullable: bool,
    pub default_value: Option<String>,
}

/// A table constraint.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Constraint {
    pub name: String,
    pub kind: ConstraintKind,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ConstraintKind {
    PrimaryKey { columns: Vec<String> },
    ForeignKey {
        columns: Vec<String>,
        ref_table: String,
        ref_columns: Vec<String>,
    },
    Unique { columns: Vec<String> },
    Check { expression: String },
}

/// An index on a table.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Index {
    pub name: String,
    pub columns: Vec<String>,
    pub is_unique: bool,
}

/// A commit in the object store.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Commit {
    pub parent_hash: Option<String>, // None for the root commit
    pub schema_hash: String,         // SHA-256 of the Schema object this commit points to
    pub timestamp: i64,              // Unix timestamp (seconds)
    pub message: String,
}

/// Every possible schema change the diff engine can produce.
/// This enum is used by: diff, merge, and migration export.
/// Adding a variant here means updating all three modules.
#[derive(Debug, Clone, PartialEq)]
pub enum DiffOp {
    // Table level
    AddTable  { name: String, table: Table },
    DropTable { name: String },

    // Column level
    AddColumn    { table: String, column: Column },
    DropColumn   { table: String, column_name: String },
    ModifyColumn { table: String, column_name: String, change: ColumnChange },
    RenameColumn { table: String, from: String, to: String }, // heuristic detection

    // Constraint level
    AddConstraint  { table: String, constraint: Constraint },
    DropConstraint { table: String, constraint_name: String },

    // Index level
    AddIndex  { table: String, index: Index },
    DropIndex { table: String, index_name: String },
}

/// Describes what specifically changed about a column.
#[derive(Debug, Clone, PartialEq)]
pub struct ColumnChange {
    pub data_type:     Option<(String, String)>,              // (old, new)
    pub is_nullable:   Option<(bool, bool)>,                  // (old, new)
    pub default_value: Option<(Option<String>, Option<String>)>, // (old, new)
}
```

**Why `ordinal_position`?**  
The rename detection heuristic depends on it. When column `email` disappears and column
`email_address` appears at the same position with the same type and constraints, it's a
rename — not a drop + add. Without position, you can't tell them apart.

**Why is `data_type` a `String` not an enum?**  
Postgres has hundreds of types including user-defined types, domains, and extensions.
Enumerating them all is not worth it in v1. Store the normalized string from
`information_schema.columns.data_type` and compare directly.

**Why hash the structured representation, not the raw SQL?**  
Two equivalent schemas can produce different raw SQL (whitespace, formatting, column
order). If you hash raw SQL, semantically identical schemas get different hashes —
content-addressing breaks down. By serializing the `Schema` struct with bincode and
hashing that, you hash the semantic content, not the syntax. This is called
canonicalization, and it's why the data model matters.

---

## 11. Story 1.1 — Your First Task

This is the first story. It should take 1–2 hours. At the end, `helix init` runs and
creates `.helix/` on disk. No database connection yet.

### What to implement

`helix init <database-url>` should:
1. Check `.helix/` does not already exist — error if it does
2. Create `.helix/config.toml` with the database URL
3. Create `.helix/objects/`
4. Create `.helix/refs/heads/`
5. Write `.helix/HEAD` containing `ref: refs/heads/main`
6. Print `Initialized Helix repository.`

### helix-core/src/lib.rs

```rust
pub mod branch;
pub mod commit;
pub mod diff;
pub mod merge;
pub mod migrate;
pub mod snapshot;
pub mod store;
pub mod types;

use anyhow::{bail, Result};
use std::fs;
use std::path::Path;

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

    println!("Initialized Helix repository.");
    Ok(())
}
```

### helix-cli/src/main.rs

```rust
use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "helix", about = "Git for database schemas", version)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Initialise a Helix repository for a database
    Init {
        /// Postgres connection URL (e.g. postgres://user:pass@localhost:5433/mydb)
        database_url: String,
    },
    /// Snapshot the current schema as a commit
    Commit {
        #[arg(short, long)]
        message: String,
    },
    /// Show commit history
    Log,
    /// Show what changed since the last commit
    Status,
    /// Show schema diff
    Diff {
        /// Branch name or commit hash (omit to diff HEAD vs live DB)
        target: Option<String>,
        /// Second target to diff two specific points
        target2: Option<String>,
    },
    /// Create or list branches
    Branch {
        /// Branch name to create (omit to list all branches)
        name: Option<String>,
    },
    /// Switch to a branch
    Checkout { name: String },
    /// Merge a branch into the current branch
    Merge { name: String },
    /// Export a SQL migration from the diff between two branches
    ExportMigration {
        from: String,
        to: String,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();

    let cli = Cli::parse();

    match cli.command {
        Commands::Init { database_url } => {
            helix_core::init(&database_url)?;
        }
        Commands::Commit { message } => {
            todo!("Story 2.4")
        }
        Commands::Log => {
            todo!("Story 2.5")
        }
        Commands::Status => {
            todo!("Story 1.6")
        }
        Commands::Diff { target, target2 } => {
            todo!("Story 3.8")
        }
        Commands::Branch { name } => {
            todo!("Story 4.1 / 4.2")
        }
        Commands::Checkout { name } => {
            todo!("Story 4.3")
        }
        Commands::Merge { name } => {
            todo!("Story 5.5")
        }
        Commands::ExportMigration { from, to } => {
            todo!("Story 6.3")
        }
    }

    Ok(())
}
```

### Verify it works

```bash
# Build
cargo build

# Run init
cargo run --bin helix -- init postgres://helix:helix@localhost:5433/helix_dev

# Expected output:
# Initialized Helix repository.

# Inspect what was created
ls -la .helix/
cat .helix/HEAD
cat .helix/config.toml

# Run again — should fail
cargo run --bin helix -- init postgres://helix:helix@localhost:5433/helix_dev
# Expected: Error: Already a Helix repository (found .helix/ in current directory)

# Clean up for the next test run
rm -rf .helix/
```

Story 1.1 is done when all of the above works correctly.

---

## 12. Running and Testing

### Running commands

```bash
cargo run --bin helix -- <subcommand> [args]

# Examples
cargo run --bin helix -- init postgres://helix:helix@localhost:5433/helix_dev
cargo run --bin helix -- commit -m "add payments table"
cargo run --bin helix -- diff feature-payments
cargo run --bin helix -- diff abc123 def456
```

### Running tests

```bash
cargo test                           # all tests in all crates
cargo test -p helix-core             # only helix-core tests
cargo test diff                      # only tests matching "diff"
cargo test -- --nocapture            # show println! output while testing
cargo test -- --test-threads=1       # run tests serially (needed if they share DB state)
```

### Integration tests (require Docker)

```bash
# Start the test database first
docker compose up -d postgres_test

# Run integration tests
TEST_DATABASE_URL=postgres://helix_test:helix_test@localhost:5434/helix_test \
    cargo test -p helix-core --test integration
```

### Debug logging

```bash
RUST_LOG=debug cargo run --bin helix -- commit -m "test"
RUST_LOG=helix_core=debug cargo run --bin helix -- diff main
```

### Reset the dev database

```bash
docker compose down -v && docker compose up -d
# -v wipes all volumes and re-runs seed.sql on next start
```

### Install helix locally (run as a system command)

```bash
cargo install --path helix-cli
helix init postgres://helix:helix@localhost:5433/helix_dev
```

---

## 13. Git Workflow for This Project

### Branch naming

```
main                  — stable, working code only — never push directly here
feat/story-1.1        — work on story 1.1
feat/story-1.3        — work on story 1.3
```

### Commit message format

Use Conventional Commits. It makes `git log` readable and will matter when you post this
project publicly.

```
feat: implement helix init directory creation
fix: handle existing .helix/ with clear error message
chore: add docker-compose for postgres dev + test
test: add unit tests for object store write/read
refactor: extract schema serialization into store.rs
docs: update KICKSTART with story 1.1 instructions
```

### PR process

- One PR per story (or two tightly coupled stories)
- Other person reviews before merging — even if it takes 10 minutes
- Never push directly to `main`
- GitHub Actions runs `cargo test` on every PR automatically

### GitHub Actions — `.github/workflows/ci.yml`

```yaml
name: CI

on:
  push:
    branches: [main]
  pull_request:

jobs:
  test:
    runs-on: ubuntu-latest

    services:
      postgres:
        image: postgres:16-alpine
        env:
          POSTGRES_USER: helix_test
          POSTGRES_PASSWORD: helix_test
          POSTGRES_DB: helix_test
        ports:
          - 5434:5432
        options: >-
          --health-cmd pg_isready
          --health-interval 10s
          --health-timeout 5s
          --health-retries 5

    steps:
      - uses: actions/checkout@v4

      - name: Install Rust stable
        uses: dtolnay/rust-toolchain@stable

      - name: Cache cargo registry
        uses: actions/cache@v4
        with:
          path: |
            ~/.cargo/registry
            ~/.cargo/git
            target
          key: ${{ runner.os }}-cargo-${{ hashFiles('Cargo.lock') }}

      - name: Build
        run: cargo build --locked

      - name: Test
        run: cargo test --locked
        env:
          TEST_DATABASE_URL: postgres://helix_test:helix_test@localhost:5434/helix_test
```

---

## 14. Common Errors and Fixes

### `error[E0432]: unresolved import`

You used something from another module without bringing it into scope.  
Fix: add `pub mod <name>;` to `lib.rs`, or add `use helix_core::<thing>;` to the file that needs it.

### `error[E0277]: the trait bound ... is not satisfied`

Most likely causes:
- Missing `#[derive(Serialize)]` on a struct you're trying to serialize
- Missing `#[derive(PartialEq)]` on a type used in an `==` comparison
- A type isn't `Send + Sync` but needs to be for async

The error message tells you exactly which trait is missing on which type. Read it.

### `error: future cannot be sent between threads safely`

A non-`Send` type is inside an async function. Most common cause: using `Rc<T>` (use
`Arc<T>` instead). Also happens when holding a raw `PgConnection` across an `.await`
instead of using a `PgPool`.

### `connection refused` to Postgres

```bash
docker compose ps                  # check if containers are running
docker compose up -d               # start them
docker compose logs postgres       # look for startup errors
```

Also check you're using port 5433 for dev (not 5432).

### `password authentication failed for user "helix"`

Your `.env` values don't match `docker-compose.yml`. Both must use the same
username (`helix`), password (`helix`), and database name (`helix_dev`).

### `cargo test` fails but `cargo run` works

Integration tests need `TEST_DATABASE_URL` pointing to port 5434. Either export it:
```bash
export TEST_DATABASE_URL=postgres://helix_test:helix_test@localhost:5434/helix_test
```
Or prefix it to the cargo command every time.

### `could not find Cargo.toml for package helix-cli`

You ran `cargo new` from inside a subdirectory, or the workspace `Cargo.toml` is missing
the `members` list. Run all commands from the project root.

### `linker 'cc' not found`

You're missing `build-essential`:
```bash
sudo apt-get install build-essential
```

### `pkg-config: not found` or OpenSSL errors

```bash
sudo apt-get install pkg-config libssl-dev
```

---

## 15. Rust Concepts You'll Hit in Week 1

These are the specific things that will confuse you as a Rust beginner. Read each one
when you encounter it — don't try to memorize them all upfront.

### The `?` operator

`?` at the end of a fallible call means: "if this returned `Err`, propagate it immediately."
Your function must also return `Result`.

```rust
// Verbose version
let contents = match fs::read_to_string("HEAD") {
    Ok(s) => s,
    Err(e) => return Err(e.into()),
};

// With ? — identical behavior
let contents = fs::read_to_string("HEAD")?;
```

### `anyhow` vs `thiserror`

- Use `anyhow::Result` in **helix-cli** — for application code that just needs to propagate
  and print errors nicely.
- Use `thiserror` in **helix-core** — for library code where callers might need to match on
  specific error types.

Define this in `helix-core/src/lib.rs`:

```rust
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
```

### `String` vs `&str`

- `String` — owned, heap-allocated. Use in structs.
- `&str` — borrowed reference. Use in function parameters when you only need to read.
- When in doubt: `.to_string()` converts `&str` → `String`.

### Clone your way through Week 1

If the borrow checker complains, add `.clone()`. It's slightly less efficient but
never wrong. Once the project works end-to-end, you can profile and remove unnecessary
clones. Don't fight the borrow checker in Week 1 — you'll understand it better after
you've shipped something.

### `async` and `.await`

Any function that calls `sqlx` must be `async`. Call async functions with `.await`.
`#[tokio::main]` on `main()` starts the runtime.

```rust
// In snapshot.rs
pub async fn extract_schema(pool: &PgPool) -> Result<Schema, HelixError> {
    let rows = sqlx::query!("SELECT ...").fetch_all(pool).await?;
    // ...
}

// In main.rs
let schema = helix_core::snapshot::extract_schema(&pool).await?;
```

### `#[derive(...)]`

Put this above every struct in `types.rs`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
```

- `Debug` — lets you print with `{:?}` for debugging
- `Clone` — lets you `.clone()` the value
- `Serialize` / `Deserialize` — lets serde + bincode convert it to/from bytes
- `PartialEq` — lets you compare with `==` (needed for diff tests)

---

## 16. How to Explain This Project

### The 2-minute architecture pitch

Practice saying this out loud until it flows naturally:

> "Helix is a Git-style version-control system for PostgreSQL database schemas. At a high
> level, a schema is represented structurally rather than as raw SQL text. Schema versions
> are stored as immutable, content-addressed objects, and commits reference those objects
> along with their parent commits, forming a DAG.
>
> Branches allow multiple schema versions to evolve independently while sharing unchanged
> objects through copy-on-write — so creating a branch costs essentially nothing.
>
> When two branches need to be combined, Helix performs a three-way merge using the
> common ancestor and the two branch states. The schema diffing layer, built with
> sqlparser-rs, compares structured SQL ASTs to produce semantic operations — AddColumn,
> DropTable, RenameColumn, and so on. Those operations can be rendered as human-readable
> colored diffs or converted into Flyway-compatible migration SQL.
>
> The full pipeline is: schema → parse and structure → content-addressed snapshot → commit
> DAG → branch → three-way merge → diff operations → migration."

### The one-sentence version

> "Helix takes PostgreSQL schemas, stores versions as immutable content-addressed objects
> in a commit DAG, lets branches share unchanged objects via copy-on-write, merges divergent
> branches with three-way merge, and converts schema differences into human-readable diffs
> and Flyway-compatible migrations."

---

## 17. Interview Attack Sequence

A technical interviewer who sees Helix on your resume will drill it in this exact order.
Prepare an answer for every question below. The ones marked ⚠ are the most likely to
trip you up.

### Layer 1 — What and why

```
What is Helix?
Why does a database schema need version control?
Why Git-style specifically?
Why PostgreSQL?
Why Rust?
```

### Layer 2 — Storage

```
What is content-addressed storage?
Why hash the content instead of using a UUID?
What hash function do you use? ← ⚠ know your exact implementation
Why are objects immutable?
What is copy-on-write?
Why does CoW save space?
How do branches work at the storage level?
What happens if two objects have the same hash? (collision question)
Why do you hash the serialized struct and not the raw SQL? ← ⚠ canonicalization
```

### Layer 3 — Data structures

```
What data structure is the commit history?
Why a DAG and not a list?
What is a directed acyclic graph?
How do you traverse it?  ← expect: DFS, BFS, topological sort
How do you find the common ancestor of two commits?
What is the complexity of your LCA algorithm?
What happens if there are multiple common ancestors?
```

### Layer 4 — Merge

```
What is three-way merge?
Why not two-way?
What is the merge base?
Walk me through the merge truth table.   ← ⚠ memorize: A/B/A→B, A/B/C→conflict
What is a conflict?
Give me a concrete conflict example.
What if both branches add the same column?   ← same def = auto-merge, diff def = conflict
What if one branch drops a table and the other modifies it?  ← ⚠ delete-vs-modify
What if both branches rename the same column differently?   ← ⚠ rename conflict
How do you represent a conflict to the user?
```

### Layer 5 — Schema and parsing

```
Why do you use sqlparser-rs?
Why not string-diff the SQL?
What is an AST?
How does your diff engine work?
What operations can your diff engine detect?   ← ⚠ know your DiffOp variants
How does rename detection work?
What is the rename heuristic?   ← same table, same type, same position, same constraints
Can the heuristic be wrong?
```

### Layer 6 — Migration

```
What is a Flyway-compatible migration?
How do you convert DiffOps to SQL?
How do you handle foreign key ordering?   ← topological sort on FK dependency graph
What if a migration fails partway through?
Are your migrations idempotent?
```

### Layer 7 — Systems (advanced)

```
How would you handle concurrent commits from two users?
How would you scale the object store beyond a single machine?
How would you garbage collect unreachable objects?
What happens if .helix/ gets corrupted?
How would you add support for MySQL?
```

### The code question

Be ready for:

> "Implement the three-way merge logic for schema nodes."  
> "Find the LCA of two commits in a DAG."  
> "Implement content-addressed object write."

For each of these, you should be able to write pseudocode or Rust in ~10 minutes.

---

## 18. Decision Log

Add a row every time you make a technical decision. Future you and future contributors
will thank you.

| Date | Decision | Rationale |
|---|---|---|
| — | Rust | Single binary distribution, strong type system for DiffOp enum, deliberate learning goal |
| — | Postgres-only in v1 | Depth over breadth — one database done correctly beats four done partially |
| — | Schema-only, no row data | Data versioning is a different problem at a different scale |
| — | Content-addressed object store | Deduplication for free, immutable history, identical to Git's model |
| — | Serialize struct → hash (not raw SQL → hash) | Canonicalization: equivalent schemas must produce identical hashes regardless of SQL formatting |
| — | AST-based diff via sqlparser-rs | String-diffing SQL produces semantic false positives — renames look like data loss |
| — | REPEATABLE READ for schema extraction | Prevents phantom reads across multiple catalog queries in a single snapshot |
| — | Two crates: helix-cli and helix-core | Separation of concerns — helix-core is independently testable without the CLI |
| — | `ordinal_position` in Column struct | Required for rename detection heuristic — position + type + constraints identify a column across a rename |
| — | Port 5433 for dev database | Avoids conflicts with any existing local Postgres on 5432 |

---

*Last updated: project start — update this doc whenever the setup process changes.*
