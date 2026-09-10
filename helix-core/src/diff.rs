use std::collections::{HashMap, HashSet};

use crate::types::{Column, ColumnChange, Constraint, DiffOp, Index, Schema, Table};

/// Compare two schemas and produce the list of operations that would turn `old` into `new`.
///
/// Operates on the structured `Schema`, not raw SQL text, so it is immune to whitespace,
/// formatting, and column-ordering noise — two schemas that are structurally identical
/// always produce the same (empty) diff, regardless of how the SQL that built them looked.
pub fn diff_schemas(old: &Schema, new: &Schema) -> Vec<DiffOp> {
    let old_tables: HashMap<&str, &Table> = old.tables.iter().map(|t| (t.name.as_str(), t)).collect();
    let new_tables: HashMap<&str, &Table> = new.tables.iter().map(|t| (t.name.as_str(), t)).collect();

    let mut table_names: Vec<&str> = old_tables
        .keys()
        .chain(new_tables.keys())
        .copied()
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    table_names.sort();

    let mut ops = Vec::new();
    for name in table_names {
        match (old_tables.get(name), new_tables.get(name)) {
            (None, Some(table)) => ops.push(DiffOp::AddTable { name: name.to_string(), table: (*table).clone() }),
            (Some(_), None) => ops.push(DiffOp::DropTable { name: name.to_string() }),
            (Some(old_table), Some(new_table)) => ops.extend(diff_table(old_table, new_table)),
            (None, None) => unreachable!("name came from the union of both key sets"),
        }
    }
    ops
}

fn diff_table(old: &Table, new: &Table) -> Vec<DiffOp> {
    let mut ops = diff_columns(&old.name, &old.columns, &new.columns);
    ops.extend(diff_constraints(&old.name, &old.constraints, &new.constraints));
    ops.extend(diff_indexes(&old.name, &old.indexes, &new.indexes));
    ops
}

/// Detect column renames between an old and new column list.
///
/// Heuristic: a column that disappears and a column that appears in the same table are
/// treated as a rename — not a drop + add — when they share the same ordinal position
/// and data type. Returns a map of old name -> new name. Shared with the merge engine,
/// which needs the same notion of "this is the same column under a new name" to reconcile
/// renames across branches.
pub(crate) fn detect_column_renames(old: &[Column], new: &[Column]) -> HashMap<String, String> {
    let old_names: HashSet<&str> = old.iter().map(|c| c.name.as_str()).collect();
    let new_names: HashSet<&str> = new.iter().map(|c| c.name.as_str()).collect();

    let mut dropped: Vec<&Column> = old.iter().filter(|c| !new_names.contains(c.name.as_str())).collect();
    let mut added: Vec<&Column> = new.iter().filter(|c| !old_names.contains(c.name.as_str())).collect();
    dropped.sort_by_key(|c| c.name.clone());
    added.sort_by_key(|c| c.name.clone());

    let mut renames = HashMap::new();
    let mut matched_added: HashSet<String> = HashSet::new();

    for d in &dropped {
        if let Some(a) = added.iter().find(|a| {
            !matched_added.contains(a.name.as_str())
                && a.ordinal_position == d.ordinal_position
                && a.data_type == d.data_type
        }) {
            matched_added.insert(a.name.clone());
            renames.insert(d.name.clone(), a.name.clone());
        }
    }

    renames
}

fn diff_columns(table: &str, old: &[Column], new: &[Column]) -> Vec<DiffOp> {
    let renames = detect_column_renames(old, new);
    let renamed_from: HashSet<&str> = renames.keys().map(String::as_str).collect();
    let renamed_to: HashSet<&str> = renames.values().map(String::as_str).collect();

    let old_by_name: HashMap<&str, &Column> = old.iter().map(|c| (c.name.as_str(), c)).collect();
    let new_by_name: HashMap<&str, &Column> = new.iter().map(|c| (c.name.as_str(), c)).collect();

    let mut ops = Vec::new();

    let mut renamed_pairs: Vec<(&String, &String)> = renames.iter().collect();
    renamed_pairs.sort();
    for (from, to) in renamed_pairs {
        ops.push(DiffOp::RenameColumn { table: table.to_string(), from: from.clone(), to: to.clone() });
    }

    let mut dropped: Vec<&str> = old_by_name
        .keys()
        .filter(|n| !new_by_name.contains_key(*n) && !renamed_from.contains(*n))
        .copied()
        .collect();
    dropped.sort();
    for name in dropped {
        ops.push(DiffOp::DropColumn { table: table.to_string(), column_name: name.to_string() });
    }

    let mut added: Vec<&str> = new_by_name
        .keys()
        .filter(|n| !old_by_name.contains_key(*n) && !renamed_to.contains(*n))
        .copied()
        .collect();
    added.sort();
    for name in added {
        ops.push(DiffOp::AddColumn { table: table.to_string(), column: new_by_name[name].clone() });
    }

    let mut common: Vec<&str> = old_by_name.keys().filter(|n| new_by_name.contains_key(*n)).copied().collect();
    common.sort();
    for name in common {
        if let Some(change) = column_change(old_by_name[name], new_by_name[name]) {
            ops.push(DiffOp::ModifyColumn { table: table.to_string(), column_name: name.to_string(), change });
        }
    }

    ops
}

fn column_change(old: &Column, new: &Column) -> Option<ColumnChange> {
    let data_type = (old.data_type != new.data_type).then(|| (old.data_type.clone(), new.data_type.clone()));
    let is_nullable = (old.is_nullable != new.is_nullable).then_some((old.is_nullable, new.is_nullable));
    let default_value = (old.default_value != new.default_value)
        .then(|| (old.default_value.clone(), new.default_value.clone()));

    if data_type.is_none() && is_nullable.is_none() && default_value.is_none() {
        None
    } else {
        Some(ColumnChange { data_type, is_nullable, default_value })
    }
}

fn diff_constraints(table: &str, old: &[Constraint], new: &[Constraint]) -> Vec<DiffOp> {
    let old_by_name: HashMap<&str, &Constraint> = old.iter().map(|c| (c.name.as_str(), c)).collect();
    let new_by_name: HashMap<&str, &Constraint> = new.iter().map(|c| (c.name.as_str(), c)).collect();

    let mut names: Vec<&str> = old_by_name
        .keys()
        .chain(new_by_name.keys())
        .copied()
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    names.sort();

    let mut ops = Vec::new();
    for name in names {
        match (old_by_name.get(name), new_by_name.get(name)) {
            (None, Some(c)) => ops.push(DiffOp::AddConstraint { table: table.to_string(), constraint: (*c).clone() }),
            (Some(_), None) => ops.push(DiffOp::DropConstraint { table: table.to_string(), constraint_name: name.to_string() }),
            (Some(o), Some(n)) if o.kind != n.kind => {
                ops.push(DiffOp::DropConstraint { table: table.to_string(), constraint_name: name.to_string() });
                ops.push(DiffOp::AddConstraint { table: table.to_string(), constraint: (*n).clone() });
            }
            _ => {}
        }
    }
    ops
}

fn diff_indexes(table: &str, old: &[Index], new: &[Index]) -> Vec<DiffOp> {
    let old_by_name: HashMap<&str, &Index> = old.iter().map(|i| (i.name.as_str(), i)).collect();
    let new_by_name: HashMap<&str, &Index> = new.iter().map(|i| (i.name.as_str(), i)).collect();

    let mut names: Vec<&str> = old_by_name
        .keys()
        .chain(new_by_name.keys())
        .copied()
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    names.sort();

    let mut ops = Vec::new();
    for name in names {
        match (old_by_name.get(name), new_by_name.get(name)) {
            (None, Some(i)) => ops.push(DiffOp::AddIndex { table: table.to_string(), index: (*i).clone() }),
            (Some(_), None) => ops.push(DiffOp::DropIndex { table: table.to_string(), index_name: name.to_string() }),
            (Some(o), Some(n)) if o != n => {
                ops.push(DiffOp::DropIndex { table: table.to_string(), index_name: name.to_string() });
                ops.push(DiffOp::AddIndex { table: table.to_string(), index: (*n).clone() });
            }
            _ => {}
        }
    }
    ops
}

#[cfg(test)]
mod tests {
    use super::*;

    fn col(name: &str, pos: i32, data_type: &str, nullable: bool) -> Column {
        Column {
            name: name.to_string(),
            ordinal_position: pos,
            data_type: data_type.to_string(),
            char_max_length: None,
            is_nullable: nullable,
            default_value: None,
        }
    }

    fn table(name: &str, columns: Vec<Column>) -> Table {
        Table { name: name.to_string(), columns, constraints: vec![], indexes: vec![] }
    }

    #[test]
    fn detects_add_table() {
        let old = Schema { tables: vec![] };
        let new = Schema { tables: vec![table("users", vec![col("id", 1, "integer", false)])] };
        let ops = diff_schemas(&old, &new);
        assert_eq!(ops, vec![DiffOp::AddTable { name: "users".into(), table: new.tables[0].clone() }]);
    }

    #[test]
    fn detects_drop_table() {
        let old = Schema { tables: vec![table("users", vec![])] };
        let new = Schema { tables: vec![] };
        let ops = diff_schemas(&old, &new);
        assert_eq!(ops, vec![DiffOp::DropTable { name: "users".into() }]);
    }

    #[test]
    fn detects_column_rename_not_drop_add() {
        let old = table("users", vec![col("email", 2, "character varying", false)]);
        let new = table("users", vec![col("email_address", 2, "character varying", false)]);
        let ops = diff_table(&old, &new);
        assert_eq!(
            ops,
            vec![DiffOp::RenameColumn { table: "users".into(), from: "email".into(), to: "email_address".into() }]
        );
    }

    #[test]
    fn different_position_or_type_is_drop_and_add_not_rename() {
        let old = table("users", vec![col("email", 2, "character varying", false)]);
        let new = table("users", vec![col("email_address", 3, "integer", false)]);
        let ops = diff_table(&old, &new);
        assert!(ops.contains(&DiffOp::DropColumn { table: "users".into(), column_name: "email".into() }));
        assert!(ops.contains(&DiffOp::AddColumn { table: "users".into(), column: new.columns[0].clone() }));
    }

    #[test]
    fn detects_modify_column() {
        let old = table("users", vec![col("age", 2, "integer", true)]);
        let new = table("users", vec![col("age", 2, "integer", false)]);
        let ops = diff_table(&old, &new);
        assert_eq!(
            ops,
            vec![DiffOp::ModifyColumn {
                table: "users".into(),
                column_name: "age".into(),
                change: ColumnChange {
                    data_type: None,
                    is_nullable: Some((true, false)),
                    default_value: None,
                },
            }]
        );
    }

    #[test]
    fn identical_schemas_produce_no_diff() {
        let schema = Schema { tables: vec![table("users", vec![col("id", 1, "integer", false)])] };
        assert!(diff_schemas(&schema, &schema).is_empty());
    }
}
