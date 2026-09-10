use anyhow::Result;
use std::collections::HashSet;

use crate::commit;
use crate::diff;
use crate::types::{Column, Constraint, Index, Schema, Table};

#[derive(Debug, Clone)]
pub struct Conflict {
    pub description: String,
}

impl Conflict {
    fn new(description: String) -> Self {
        Self { description }
    }
}

pub struct MergeResult {
    pub schema: Schema,
    pub conflicts: Vec<Conflict>,
}

impl MergeResult {
    pub fn is_clean(&self) -> bool {
        self.conflicts.is_empty()
    }
}

/// Find the most recent commit that is an ancestor of both `hash_a` and `hash_b`,
/// by walking `hash_a`'s ancestry into a set and then walking `hash_b`'s ancestry
/// until the first hit. Commits form a simple parent chain per branch (no merge
/// commit carries two parents), so this is a straight linked-list intersection.
pub fn find_lca(hash_a: &str, hash_b: &str) -> Result<Option<String>> {
    let mut ancestors_a = HashSet::new();
    let mut current = Some(hash_a.to_string());
    while let Some(hash) = current {
        ancestors_a.insert(hash.clone());
        current = commit::load_commit(&hash)?.parent_hash;
    }

    let mut current = Some(hash_b.to_string());
    while let Some(hash) = current {
        if ancestors_a.contains(&hash) {
            return Ok(Some(hash));
        }
        current = commit::load_commit(&hash)?.parent_hash;
    }
    Ok(None)
}

/// Three-way merge of two schemas against their common ancestor `base`.
///
/// Applies the merge truth table at the level of individual tables, columns,
/// constraints, and indexes: unchanged-on-one-side always yields the other side's
/// value, changed-identically-on-both auto-merges, and changed-differently conflicts.
/// Column renames are reconciled using the same heuristic the diff engine uses, so a
/// column renamed on only one branch merges cleanly, while incompatible renames of the
/// same column on both branches conflict.
pub fn merge_schemas(base: &Schema, ours: &Schema, theirs: &Schema) -> MergeResult {
    let mut names: Vec<String> = base
        .tables
        .iter()
        .chain(&ours.tables)
        .chain(&theirs.tables)
        .map(|t| t.name.clone())
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    names.sort();

    let find = |schema: &Schema, name: &str| schema.tables.iter().find(|t| t.name == name).cloned();

    let mut tables = Vec::new();
    let mut conflicts = Vec::new();

    for name in names {
        let base_t = find(base, &name);
        let ours_t = find(ours, &name);
        let theirs_t = find(theirs, &name);

        match (&base_t, &ours_t, &theirs_t) {
            (_, None, None) => {}
            (None, Some(o), None) => tables.push(o.clone()),
            (None, None, Some(t)) => tables.push(t.clone()),
            (None, Some(o), Some(t)) => {
                if o == t {
                    tables.push(o.clone());
                } else {
                    conflicts.push(Conflict::new(format!(
                        "table '{name}': added independently on both branches with different definitions"
                    )));
                }
            }
            (Some(b), Some(o), None) => {
                if o != b {
                    conflicts.push(Conflict::new(format!(
                        "table '{name}': dropped on one branch but modified on the other"
                    )));
                }
                // else: theirs dropped it, ours left it unchanged -> drop, nothing to push
            }
            (Some(b), None, Some(t)) => {
                if t != b {
                    conflicts.push(Conflict::new(format!(
                        "table '{name}': dropped on one branch but modified on the other"
                    )));
                }
                // else: ours dropped it -> drop
            }
            (Some(b), Some(o), Some(t)) => {
                let (columns, col_conflicts) = merge_columns(&name, &b.columns, &o.columns, &t.columns);
                let (constraints, con_conflicts) = merge_set(&b.constraints, &o.constraints, &t.constraints, |c: &Constraint| {
                    c.name.as_str()
                });
                let (indexes, idx_conflicts) =
                    merge_set(&b.indexes, &o.indexes, &t.indexes, |i: &Index| i.name.as_str());

                conflicts.extend(col_conflicts);
                conflicts.extend(con_conflicts.into_iter().map(|c| Conflict::new(format!("table '{name}': {}", c.description))));
                conflicts.extend(idx_conflicts.into_iter().map(|c| Conflict::new(format!("table '{name}': {}", c.description))));

                tables.push(Table { name, columns, constraints, indexes });
            }
        }
    }

    MergeResult { schema: Schema { tables }, conflicts }
}

/// Generic three-way merge of a named, cloneable, comparable collection (constraints,
/// indexes — anything keyed by a unique `name`). Implements the full merge truth table:
/// same-on-both -> keep, changed-on-one -> take the change, changed-differently -> conflict.
fn merge_set<T, F>(base: &[T], ours: &[T], theirs: &[T], name_of: F) -> (Vec<T>, Vec<Conflict>)
where
    T: Clone + PartialEq,
    F: Fn(&T) -> &str,
{
    let mut names: Vec<&str> = base
        .iter()
        .chain(ours)
        .chain(theirs)
        .map(|item| name_of(item))
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    names.sort();

    let mut result = Vec::new();
    let mut conflicts = Vec::new();

    for name in names {
        let b = find_by_name(base, name, &name_of);
        let o = find_by_name(ours, name, &name_of);
        let t = find_by_name(theirs, name, &name_of);

        match (b, o, t) {
            (_, None, None) => {}
            (None, Some(o), None) => result.push(o.clone()),
            (None, None, Some(t)) => result.push(t.clone()),
            (None, Some(o), Some(t)) => {
                if o == t {
                    result.push(o.clone());
                } else {
                    conflicts.push(Conflict::new(format!("'{name}': added independently on both branches with different definitions")));
                }
            }
            (Some(b), Some(o), None) => {
                if o != b {
                    conflicts.push(Conflict::new(format!("'{name}': dropped on one branch but modified on the other")));
                }
            }
            (Some(b), None, Some(t)) => {
                if t != b {
                    conflicts.push(Conflict::new(format!("'{name}': dropped on one branch but modified on the other")));
                }
            }
            (Some(b), Some(o), Some(t)) => {
                if o == t {
                    result.push(o.clone());
                } else if o == b {
                    result.push(t.clone());
                } else if t == b {
                    result.push(o.clone());
                } else {
                    conflicts.push(Conflict::new(format!("'{name}': modified differently on both branches")));
                }
            }
        }
    }

    (result, conflicts)
}

fn find_by_name<'a, T>(items: &'a [T], name: &str, name_of: &impl Fn(&T) -> &str) -> Option<&'a T> {
    items.iter().find(|item| name_of(item) == name)
}

/// Three-way column merge. Columns are matched by their *base* identity, not their
/// literal current name, so that a column renamed on only one branch is recognized as
/// the same column rather than a drop + unrelated add — and two branches renaming the
/// same base column to different names correctly conflicts.
fn merge_columns(table: &str, base: &[Column], ours: &[Column], theirs: &[Column]) -> (Vec<Column>, Vec<Conflict>) {
    let renames_ours = diff::detect_column_renames(base, ours);
    let renames_theirs = diff::detect_column_renames(base, theirs);

    let find_by_name = |cols: &[Column], name: &str| cols.iter().find(|c| c.name == name).cloned();

    let base_names: HashSet<&str> = base.iter().map(|c| c.name.as_str()).collect();
    let renamed_ours_targets: HashSet<&str> = renames_ours.values().map(String::as_str).collect();
    let renamed_theirs_targets: HashSet<&str> = renames_theirs.values().map(String::as_str).collect();

    let mut canonical_ids: Vec<String> = base.iter().map(|c| c.name.clone()).collect();
    for c in ours.iter().chain(theirs.iter()) {
        let is_new_identity = !base_names.contains(c.name.as_str())
            && !renamed_ours_targets.contains(c.name.as_str())
            && !renamed_theirs_targets.contains(c.name.as_str())
            && !canonical_ids.contains(&c.name);
        if is_new_identity {
            canonical_ids.push(c.name.clone());
        }
    }

    let mut result = Vec::new();
    let mut conflicts = Vec::new();

    for id in canonical_ids {
        let base_col = find_by_name(base, &id);
        let ours_name = renames_ours.get(&id).cloned().unwrap_or_else(|| id.clone());
        let theirs_name = renames_theirs.get(&id).cloned().unwrap_or_else(|| id.clone());
        let ours_col = find_by_name(ours, &ours_name);
        let theirs_col = find_by_name(theirs, &theirs_name);

        match (&base_col, &ours_col, &theirs_col) {
            (_, None, None) => {}
            (None, Some(o), None) => result.push(o.clone()),
            (None, None, Some(t)) => result.push(t.clone()),
            (None, Some(o), Some(t)) => {
                if o == t {
                    result.push(o.clone());
                } else {
                    conflicts.push(Conflict::new(format!(
                        "{table}.{id}: added independently on both branches with different definitions"
                    )));
                }
            }
            (Some(b), Some(o), None) => {
                if o != b {
                    conflicts.push(Conflict::new(format!("{table}.{id}: dropped on one branch but changed on the other")));
                }
            }
            (Some(b), None, Some(t)) => {
                if t != b {
                    conflicts.push(Conflict::new(format!("{table}.{id}: dropped on one branch but changed on the other")));
                }
            }
            (Some(b), Some(o), Some(t)) => {
                if o == t {
                    result.push(o.clone());
                    continue;
                }

                let renamed_ours = ours_name != id;
                let renamed_theirs = theirs_name != id;
                if renamed_ours && renamed_theirs && ours_name != theirs_name {
                    conflicts.push(Conflict::new(format!(
                        "{table}.{id}: renamed differently on both branches ('{ours_name}' vs '{theirs_name}')"
                    )));
                    continue;
                }
                let final_name = if renamed_ours { ours_name.clone() } else { theirs_name.clone() };

                match merge_column_fields(b, o, t) {
                    Ok(mut merged) => {
                        merged.name = final_name;
                        result.push(merged);
                    }
                    Err(()) => {
                        conflicts.push(Conflict::new(format!("{table}.{id}: modified differently on both branches")));
                    }
                }
            }
        }
    }

    (result, conflicts)
}

fn merge_column_fields(base: &Column, ours: &Column, theirs: &Column) -> Result<Column, ()> {
    Ok(Column {
        name: ours.name.clone(), // caller overwrites with the reconciled name
        ordinal_position: merge_value(&base.ordinal_position, &ours.ordinal_position, &theirs.ordinal_position)?,
        data_type: merge_value(&base.data_type, &ours.data_type, &theirs.data_type)?,
        char_max_length: merge_value(&base.char_max_length, &ours.char_max_length, &theirs.char_max_length)?,
        is_nullable: merge_value(&base.is_nullable, &ours.is_nullable, &theirs.is_nullable)?,
        default_value: merge_value(&base.default_value, &ours.default_value, &theirs.default_value)?,
    })
}

/// The scalar case of the merge truth table: same-on-both -> keep, changed-on-one ->
/// take the change, changed-differently -> conflict (`Err`).
fn merge_value<T: Clone + PartialEq>(base: &T, ours: &T, theirs: &T) -> Result<T, ()> {
    if ours == theirs {
        Ok(ours.clone())
    } else if ours == base {
        Ok(theirs.clone())
    } else if theirs == base {
        Ok(ours.clone())
    } else {
        Err(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn col(name: &str, pos: i32, data_type: &str) -> Column {
        Column {
            name: name.to_string(),
            ordinal_position: pos,
            data_type: data_type.to_string(),
            char_max_length: None,
            is_nullable: false,
            default_value: None,
        }
    }

    fn schema(tables: Vec<Table>) -> Schema {
        Schema { tables }
    }

    fn table(name: &str, columns: Vec<Column>) -> Table {
        Table { name: name.to_string(), columns, constraints: vec![], indexes: vec![] }
    }

    #[test]
    fn only_ours_changed_takes_ours() {
        let base = schema(vec![table("users", vec![col("id", 1, "integer")])]);
        let ours = schema(vec![table("users", vec![col("id", 1, "integer"), col("email", 2, "text")])]);
        let theirs = base.clone();
        let result = merge_schemas(&base, &ours, &theirs);
        assert!(result.is_clean());
        assert_eq!(result.schema, ours);
    }

    #[test]
    fn both_add_same_column_same_definition_auto_merges() {
        let base = schema(vec![table("users", vec![col("id", 1, "integer")])]);
        let ours = schema(vec![table("users", vec![col("id", 1, "integer"), col("email", 2, "varchar")])]);
        let theirs = ours.clone();
        let result = merge_schemas(&base, &ours, &theirs);
        assert!(result.is_clean());
        assert_eq!(result.schema, ours);
    }

    #[test]
    fn both_add_same_column_different_definition_conflicts() {
        let base = schema(vec![table("users", vec![col("id", 1, "integer")])]);
        let ours = schema(vec![table("users", vec![col("id", 1, "integer"), col("email", 2, "varchar")])]);
        let theirs = schema(vec![table("users", vec![col("id", 1, "integer"), col("email", 2, "text")])]);
        let result = merge_schemas(&base, &ours, &theirs);
        assert!(!result.is_clean());
    }

    #[test]
    fn drop_vs_modify_conflicts() {
        let base = schema(vec![table("users", vec![col("id", 1, "integer")])]);
        let ours = schema(vec![]); // dropped the table
        let theirs = schema(vec![table("users", vec![col("id", 1, "integer"), col("email", 2, "text")])]);
        let result = merge_schemas(&base, &ours, &theirs);
        assert!(!result.is_clean());
    }

    #[test]
    fn rename_vs_rename_conflicts() {
        let base = schema(vec![table("users", vec![col("username", 1, "text")])]);
        let ours = schema(vec![table("users", vec![col("user_name", 1, "text")])]);
        let theirs = schema(vec![table("users", vec![col("login_name", 1, "text")])]);
        let result = merge_schemas(&base, &ours, &theirs);
        assert!(!result.is_clean());
    }

    #[test]
    fn rename_on_one_branch_only_merges_cleanly() {
        let base = schema(vec![table("users", vec![col("username", 1, "text")])]);
        let ours = schema(vec![table("users", vec![col("user_name", 1, "text")])]);
        let theirs = base.clone();
        let result = merge_schemas(&base, &ours, &theirs);
        assert!(result.is_clean());
        assert_eq!(result.schema, ours);
    }
}
