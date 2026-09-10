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
