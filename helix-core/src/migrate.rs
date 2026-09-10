use crate::types::{Column, Constraint, ConstraintKind, DiffOp, Index};

/// Render a list of diff operations as SQL statements.
///
/// Ordering: drops, then renames, then modifies, then creates, with foreign key
/// constraints always emitted last — after every `CREATE TABLE` / `ADD COLUMN` /
/// other `ADD CONSTRAINT` statement — so a table is guaranteed to exist by the time
/// anything references it. This sidesteps needing a general topological sort of table
/// creation order: creating every table first, then wiring up every FK afterward, is
/// itself a valid (and much simpler) topological order of the dependency graph.
pub fn to_sql(ops: &[DiffOp]) -> Vec<String> {
    let mut drops = Vec::new();
    let mut renames = Vec::new();
    let mut modifies = Vec::new();
    let mut creates = Vec::new();
    let mut foreign_keys = Vec::new();

    for op in ops {
        match op {
            DiffOp::AddTable { table, .. } => {
                creates.push(create_table_sql(table.name.as_str(), &table.columns, &table.constraints));
                foreign_keys.extend(foreign_key_sql(&table.name, &table.constraints));
            }
            DiffOp::DropTable { name } => {
                drops.push(format!("DROP TABLE {};", quote_ident(name)));
            }
            DiffOp::AddColumn { table, column } => {
                creates.push(format!("ALTER TABLE {} ADD COLUMN {};", quote_ident(table), column_def_sql(column)));
            }
            DiffOp::DropColumn { table, column_name } => {
                drops.push(format!("ALTER TABLE {} DROP COLUMN {};", quote_ident(table), quote_ident(column_name)));
            }
            DiffOp::RenameColumn { table, from, to } => {
                renames.push(format!(
                    "ALTER TABLE {} RENAME COLUMN {} TO {};",
                    quote_ident(table),
                    quote_ident(from),
                    quote_ident(to)
                ));
            }
            DiffOp::ModifyColumn { table, column_name, change } => {
                modifies.extend(modify_column_sql(table, column_name, change));
            }
            DiffOp::AddConstraint { table, constraint } => {
                let sql = format!(
                    "ALTER TABLE {} ADD CONSTRAINT {} {};",
                    quote_ident(table),
                    quote_ident(&constraint.name),
                    constraint_def_sql(&constraint.kind)
                );
                if matches!(constraint.kind, ConstraintKind::ForeignKey { .. }) {
                    foreign_keys.push(sql);
                } else {
                    creates.push(sql);
                }
            }
            DiffOp::DropConstraint { table, constraint_name } => {
                drops.push(format!(
                    "ALTER TABLE {} DROP CONSTRAINT {};",
                    quote_ident(table),
                    quote_ident(constraint_name)
                ));
            }
            DiffOp::AddIndex { table, index } => {
                creates.push(create_index_sql(table, index));
            }
            DiffOp::DropIndex { index_name, .. } => {
                drops.push(format!("DROP INDEX {};", quote_ident(index_name)));
            }
        }
    }

    let mut statements = Vec::new();
    statements.extend(drops);
    statements.extend(renames);
    statements.extend(modifies);
    statements.extend(creates);
    statements.extend(foreign_keys);
    statements
}

fn create_table_sql(name: &str, columns: &[Column], constraints: &[Constraint]) -> String {
    let mut lines: Vec<String> = columns.iter().map(column_def_sql).collect();
    for constraint in constraints {
        // Foreign keys are added separately, once every table in the migration exists.
        if matches!(constraint.kind, ConstraintKind::ForeignKey { .. }) {
            continue;
        }
        lines.push(format!("CONSTRAINT {} {}", quote_ident(&constraint.name), constraint_def_sql(&constraint.kind)));
    }
    format!("CREATE TABLE {} (\n    {}\n);", quote_ident(name), lines.join(",\n    "))
}

fn foreign_key_sql(table: &str, constraints: &[Constraint]) -> Vec<String> {
    constraints
        .iter()
        .filter(|c| matches!(c.kind, ConstraintKind::ForeignKey { .. }))
        .map(|c| {
            format!(
                "ALTER TABLE {} ADD CONSTRAINT {} {};",
                quote_ident(table),
                quote_ident(&c.name),
                constraint_def_sql(&c.kind)
            )
        })
        .collect()
}

fn create_index_sql(table: &str, index: &Index) -> String {
    let unique = if index.is_unique { "UNIQUE " } else { "" };
    format!(
        "CREATE {unique}INDEX {} ON {} ({});",
        quote_ident(&index.name),
        quote_ident(table),
        quote_idents(&index.columns)
    )
}

fn column_def_sql(column: &Column) -> String {
    let mut sql = format!("{} {}", quote_ident(&column.name), sql_type(column));
    if !column.is_nullable {
        sql.push_str(" NOT NULL");
    }
    if let Some(default) = &column.default_value {
        sql.push_str(&format!(" DEFAULT {default}"));
    }
    sql
}

fn sql_type(column: &Column) -> String {
    match (column.data_type.as_str(), column.char_max_length) {
        ("character varying", Some(len)) => format!("VARCHAR({len})"),
        ("character", Some(len)) => format!("CHAR({len})"),
        (other, _) => other.to_uppercase(),
    }
}

fn constraint_def_sql(kind: &ConstraintKind) -> String {
    match kind {
        ConstraintKind::PrimaryKey { columns } => format!("PRIMARY KEY ({})", quote_idents(columns)),
        ConstraintKind::Unique { columns } => format!("UNIQUE ({})", quote_idents(columns)),
        ConstraintKind::ForeignKey { columns, ref_table, ref_columns } => format!(
            "FOREIGN KEY ({}) REFERENCES {} ({})",
            quote_idents(columns),
            quote_ident(ref_table),
            quote_idents(ref_columns)
        ),
        ConstraintKind::Check { expression } => format!("CHECK ({expression})"),
    }
}

fn modify_column_sql(table: &str, column_name: &str, change: &crate::types::ColumnChange) -> Vec<String> {
    let mut statements = Vec::new();

    if let Some((_, new_type)) = &change.data_type {
        statements.push(format!(
            "ALTER TABLE {} ALTER COLUMN {} TYPE {};",
            quote_ident(table),
            quote_ident(column_name),
            new_type.to_uppercase()
        ));
    }
    if let Some((_, new_nullable)) = change.is_nullable {
        let clause = if new_nullable { "DROP NOT NULL" } else { "SET NOT NULL" };
        statements.push(format!("ALTER TABLE {} ALTER COLUMN {} {};", quote_ident(table), quote_ident(column_name), clause));
    }
    if let Some((_, new_default)) = &change.default_value {
        let clause = match new_default {
            Some(d) => format!("SET DEFAULT {d}"),
            None => "DROP DEFAULT".to_string(),
        };
        statements.push(format!("ALTER TABLE {} ALTER COLUMN {} {};", quote_ident(table), quote_ident(column_name), clause));
    }

    statements
}

fn quote_idents(names: &[String]) -> String {
    names.iter().map(|n| quote_ident(n)).collect::<Vec<_>>().join(", ")
}

fn quote_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Table;

    #[test]
    fn add_table_defers_foreign_keys_after_all_creates() {
        let orders = Table {
            name: "orders".into(),
            columns: vec![Column {
                name: "user_id".into(),
                ordinal_position: 1,
                data_type: "integer".into(),
                char_max_length: None,
                is_nullable: false,
                default_value: None,
            }],
            constraints: vec![Constraint {
                name: "orders_user_id_fkey".into(),
                kind: ConstraintKind::ForeignKey {
                    columns: vec!["user_id".into()],
                    ref_table: "users".into(),
                    ref_columns: vec!["id".into()],
                },
            }],
            indexes: vec![],
        };
        let users = Table {
            name: "users".into(),
            columns: vec![Column {
                name: "id".into(),
                ordinal_position: 1,
                data_type: "integer".into(),
                char_max_length: None,
                is_nullable: false,
                default_value: None,
            }],
            constraints: vec![],
            indexes: vec![],
        };

        // Deliberately diff-ordered so the referencing table comes first — the FK
        // must still land after both CREATE TABLE statements.
        let ops = vec![
            DiffOp::AddTable { name: "orders".into(), table: orders },
            DiffOp::AddTable { name: "users".into(), table: users },
        ];
        let sql = to_sql(&ops);

        let orders_create = sql.iter().position(|s| s.starts_with("CREATE TABLE \"orders\"")).unwrap();
        let users_create = sql.iter().position(|s| s.starts_with("CREATE TABLE \"users\"")).unwrap();
        let fk = sql.iter().position(|s| s.contains("ADD CONSTRAINT \"orders_user_id_fkey\"")).unwrap();

        assert!(fk > orders_create);
        assert!(fk > users_create);
    }
}
