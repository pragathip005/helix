use anyhow::Result;
use sqlx::postgres::{PgPoolOptions, Postgres};
use sqlx::{PgPool, Row, Transaction};
use std::collections::HashMap;

use crate::types::{Column, Constraint, ConstraintKind, Index, Schema, Table};

/// Open a connection pool to the given Postgres database.
pub async fn connect(database_url: &str) -> Result<PgPool> {
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(database_url)
        .await?;
    Ok(pool)
}

/// Extract the full schema of the `public` schema from a live Postgres database.
///
/// Every catalog query runs inside a single REPEATABLE READ transaction so the
/// snapshot is consistent even if another session changes the schema concurrently.
pub async fn extract_schema(pool: &PgPool) -> Result<Schema> {
    let mut tx = pool.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ")
        .execute(&mut *tx)
        .await?;

    let table_names = fetch_table_names(&mut tx).await?;
    let mut columns = fetch_columns(&mut tx).await?;
    let mut primary_keys = fetch_key_constraints(&mut tx, "PRIMARY KEY").await?;
    let mut uniques = fetch_key_constraints(&mut tx, "UNIQUE").await?;
    let mut foreign_keys = fetch_foreign_keys(&mut tx).await?;
    let mut checks = fetch_check_constraints(&mut tx).await?;
    let mut indexes = fetch_indexes(&mut tx).await?;

    tx.commit().await?;

    let tables = table_names
        .into_iter()
        .map(|name| {
            let mut constraints = Vec::new();
            constraints.extend(primary_keys.remove(&name).unwrap_or_default());
            constraints.extend(uniques.remove(&name).unwrap_or_default());
            constraints.extend(foreign_keys.remove(&name).unwrap_or_default());
            constraints.extend(checks.remove(&name).unwrap_or_default());

            Table {
                columns: columns.remove(&name).unwrap_or_default(),
                indexes: indexes.remove(&name).unwrap_or_default(),
                constraints,
                name,
            }
        })
        .collect();

    Ok(Schema { tables })
}

async fn fetch_table_names(tx: &mut Transaction<'_, Postgres>) -> Result<Vec<String>> {
    let rows = sqlx::query(
        "SELECT table_name FROM information_schema.tables \
         WHERE table_schema = 'public' AND table_type = 'BASE TABLE' \
         ORDER BY table_name",
    )
    .fetch_all(&mut **tx)
    .await?;

    Ok(rows.iter().map(|r| r.get::<String, _>("table_name")).collect())
}

async fn fetch_columns(tx: &mut Transaction<'_, Postgres>) -> Result<HashMap<String, Vec<Column>>> {
    let rows = sqlx::query(
        "SELECT table_name, column_name, ordinal_position, data_type, \
                character_maximum_length, is_nullable, column_default \
         FROM information_schema.columns \
         WHERE table_schema = 'public' \
         ORDER BY table_name, ordinal_position",
    )
    .fetch_all(&mut **tx)
    .await?;

    let mut by_table: HashMap<String, Vec<Column>> = HashMap::new();
    for row in rows {
        let table_name: String = row.get("table_name");
        let column = Column {
            name: row.get("column_name"),
            ordinal_position: row.get("ordinal_position"),
            data_type: row.get("data_type"),
            char_max_length: row.get("character_maximum_length"),
            is_nullable: row.get::<String, _>("is_nullable") == "YES",
            default_value: row.get("column_default"),
        };
        by_table.entry(table_name).or_default().push(column);
    }
    Ok(by_table)
}

/// Fetches PRIMARY KEY or UNIQUE constraints (same shape: a named set of columns).
/// Relies on the query's ORDER BY to keep each constraint's rows contiguous, so a
/// constraint spanning multiple columns is merged into a single `Constraint` here
/// rather than one per column.
async fn fetch_key_constraints(
    tx: &mut Transaction<'_, Postgres>,
    constraint_type: &str,
) -> Result<HashMap<String, Vec<Constraint>>> {
    let rows = sqlx::query(
        "SELECT tc.table_name, tc.constraint_name, kcu.column_name \
         FROM information_schema.table_constraints tc \
         JOIN information_schema.key_column_usage kcu \
           ON tc.constraint_name = kcu.constraint_name \
          AND tc.table_schema = kcu.table_schema \
         WHERE tc.table_schema = 'public' AND tc.constraint_type = $1 \
         ORDER BY tc.table_name, tc.constraint_name, kcu.ordinal_position",
    )
    .bind(constraint_type)
    .fetch_all(&mut **tx)
    .await?;

    let mut by_table: HashMap<String, Vec<Constraint>> = HashMap::new();
    for row in rows {
        let table_name: String = row.get("table_name");
        let constraint_name: String = row.get("constraint_name");
        let column_name: String = row.get("column_name");

        let constraints = by_table.entry(table_name).or_default();
        match constraints.last_mut() {
            Some(c) if c.name == constraint_name => match &mut c.kind {
                ConstraintKind::PrimaryKey { columns } | ConstraintKind::Unique { columns } => {
                    columns.push(column_name);
                }
                _ => unreachable!("grouped by constraint_type"),
            },
            _ => {
                let kind = if constraint_type == "PRIMARY KEY" {
                    ConstraintKind::PrimaryKey { columns: vec![column_name] }
                } else {
                    ConstraintKind::Unique { columns: vec![column_name] }
                };
                constraints.push(Constraint { name: constraint_name, kind });
            }
        }
    }
    Ok(by_table)
}

/// Note: for a composite foreign key, this pairs local and referenced columns purely
/// by row order out of `constraint_column_usage`, which is not guaranteed to match
/// positional order for multi-column keys. Fine for the common single-column case;
/// revisit if composite FKs need to be modeled precisely.
async fn fetch_foreign_keys(tx: &mut Transaction<'_, Postgres>) -> Result<HashMap<String, Vec<Constraint>>> {
    let rows = sqlx::query(
        "SELECT tc.table_name, tc.constraint_name, kcu.column_name, \
                ccu.table_name AS ref_table, ccu.column_name AS ref_column \
         FROM information_schema.table_constraints tc \
         JOIN information_schema.key_column_usage kcu \
           ON tc.constraint_name = kcu.constraint_name \
          AND tc.table_schema = kcu.table_schema \
         JOIN information_schema.constraint_column_usage ccu \
           ON tc.constraint_name = ccu.constraint_name \
          AND tc.table_schema = ccu.table_schema \
         WHERE tc.table_schema = 'public' AND tc.constraint_type = 'FOREIGN KEY' \
         ORDER BY tc.table_name, tc.constraint_name, kcu.ordinal_position",
    )
    .fetch_all(&mut **tx)
    .await?;

    let mut by_table: HashMap<String, Vec<Constraint>> = HashMap::new();
    for row in rows {
        let table_name: String = row.get("table_name");
        let constraint_name: String = row.get("constraint_name");
        let column_name: String = row.get("column_name");
        let ref_table: String = row.get("ref_table");
        let ref_column: String = row.get("ref_column");

        let constraints = by_table.entry(table_name).or_default();
        match constraints.last_mut() {
            Some(c) if c.name == constraint_name => {
                if let ConstraintKind::ForeignKey { columns, ref_columns, .. } = &mut c.kind {
                    columns.push(column_name);
                    ref_columns.push(ref_column);
                }
            }
            _ => {
                constraints.push(Constraint {
                    name: constraint_name,
                    kind: ConstraintKind::ForeignKey {
                        columns: vec![column_name],
                        ref_table,
                        ref_columns: vec![ref_column],
                    },
                });
            }
        }
    }
    Ok(by_table)
}

async fn fetch_check_constraints(tx: &mut Transaction<'_, Postgres>) -> Result<HashMap<String, Vec<Constraint>>> {
    let rows = sqlx::query(
        "SELECT tc.table_name, tc.constraint_name, cc.check_clause \
         FROM information_schema.table_constraints tc \
         JOIN information_schema.check_constraints cc \
           ON tc.constraint_name = cc.constraint_name \
          AND tc.constraint_schema = cc.constraint_schema \
         WHERE tc.table_schema = 'public' AND tc.constraint_type = 'CHECK' \
         ORDER BY tc.table_name, tc.constraint_name",
    )
    .fetch_all(&mut **tx)
    .await?;

    let mut by_table: HashMap<String, Vec<Constraint>> = HashMap::new();
    for row in rows {
        let table_name: String = row.get("table_name");
        by_table.entry(table_name).or_default().push(Constraint {
            name: row.get("constraint_name"),
            kind: ConstraintKind::Check { expression: row.get("check_clause") },
        });
    }
    Ok(by_table)
}

/// Explicit `CREATE INDEX` indexes only — excludes the primary key index and any
/// index backing a constraint (unique/FK), since those are already represented by
/// their `Constraint` entry.
async fn fetch_indexes(tx: &mut Transaction<'_, Postgres>) -> Result<HashMap<String, Vec<Index>>> {
    let rows = sqlx::query(
        "SELECT t.relname AS table_name, i.relname AS index_name, ix.indisunique AS is_unique, \
                array_agg(a.attname ORDER BY x.n) AS columns \
         FROM pg_index ix \
         JOIN pg_class i ON i.oid = ix.indexrelid \
         JOIN pg_class t ON t.oid = ix.indrelid \
         JOIN pg_namespace n ON n.oid = t.relnamespace \
         JOIN unnest(ix.indkey::int2[]) WITH ORDINALITY AS x(attnum, n) ON true \
         JOIN pg_attribute a ON a.attrelid = t.oid AND a.attnum = x.attnum \
         LEFT JOIN pg_constraint c ON c.conindid = ix.indexrelid \
         WHERE n.nspname = 'public' AND NOT ix.indisprimary AND c.oid IS NULL \
         GROUP BY t.relname, i.relname, ix.indisunique \
         ORDER BY t.relname, i.relname",
    )
    .fetch_all(&mut **tx)
    .await?;

    let mut by_table: HashMap<String, Vec<Index>> = HashMap::new();
    for row in rows {
        let table_name: String = row.get("table_name");
        by_table.entry(table_name).or_default().push(Index {
            name: row.get("index_name"),
            columns: row.get("columns"),
            is_unique: row.get("is_unique"),
        });
    }
    Ok(by_table)
}
