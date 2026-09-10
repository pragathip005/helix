use anyhow::{bail, Result};
use clap::{Parser, Subcommand};
use colored::Colorize;
use helix_core::types::DiffOp;

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
    ExportMigration { from: String, to: String },
}

#[tokio::main]
async fn main() -> Result<()> {
    // RUST_LOG=debug (or helix_core=debug, etc. — see KICKSTART §12) controls verbosity.
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_target(false)
        .init();

    dotenvy::dotenv().ok();

    if let Err(err) = run().await {
        tracing::error!(error = %err, "command failed");
        return Err(err);
    }
    Ok(())
}

async fn run() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Init { database_url } => {
            helix_core::init(&database_url)?;
        }
        Commands::Commit { message } => {
            let schema = live_schema().await?;
            let head_schema = head_schema_or_empty()?;

            if helix_core::diff::diff_schemas(&head_schema, &schema).is_empty() {
                println!("nothing to commit, working tree clean");
                return Ok(());
            }

            let hash = helix_core::commit::create_commit(&schema, &message)?;
            println!("[{}] {}", &hash[..8], message);
        }
        Commands::Log => {
            let history = helix_core::commit::log()?;
            if history.is_empty() {
                println!("No commits yet.");
            }
            for (hash, commit) in history {
                println!("{} {}", format!("commit {hash}").yellow(), "");
                println!("Date: {}", commit.timestamp);
                println!("\n    {}\n", commit.message);
            }
        }
        Commands::Status => {
            let branch = helix_core::commit::current_branch()?;
            println!("On branch {branch}");

            if helix_core::commit::head_commit_hash()?.is_none() {
                println!("No commits yet.\n");
            }
            let head_schema = head_schema_or_empty()?;
            let live = live_schema().await?;
            let ops = helix_core::diff::diff_schemas(&head_schema, &live);
            if ops.is_empty() {
                println!("Nothing to commit, working tree clean.");
            } else {
                println!("Changes not yet committed:");
                print_diff(&ops);
            }
        }
        Commands::Diff { target, target2 } => {
            let (old, new) = match (target, target2) {
                (None, None) => (head_schema_or_empty()?, live_schema().await?),
                (Some(t), None) => (helix_core::commit::resolve_schema(&t)?, live_schema().await?),
                (Some(t1), Some(t2)) => {
                    (helix_core::commit::resolve_schema(&t1)?, helix_core::commit::resolve_schema(&t2)?)
                }
                (None, Some(_)) => bail!("a second diff target requires a first one too"),
            };
            print_diff(&helix_core::diff::diff_schemas(&old, &new));
        }
        Commands::Branch { name } => match name {
            None => {
                let current = helix_core::commit::current_branch()?;
                for (branch, hash) in helix_core::branch::list()? {
                    let marker = if branch == current { "*".green() } else { " ".normal() };
                    println!("{marker} {branch} ({})", &hash[..8.min(hash.len())]);
                }
            }
            Some(name) => {
                helix_core::branch::create(&name)?;
                println!("Created branch '{name}'");
            }
        },
        Commands::Checkout { name } => {
            helix_core::branch::checkout(&name)?;
            println!("Switched to branch '{name}'");
        }
        Commands::Merge { name } => {
            let current_branch = helix_core::commit::current_branch()?;
            let ours_hash = helix_core::commit::head_commit_hash()?
                .ok_or_else(|| anyhow::anyhow!("no commits yet on '{current_branch}'"))?;
            let theirs_hash = helix_core::commit::branch_head_hash(&name)?;

            let lca = helix_core::merge::find_lca(&ours_hash, &theirs_hash)?
                .ok_or_else(|| anyhow::anyhow!("no common ancestor between '{current_branch}' and '{name}'"))?;

            let base = helix_core::commit::load_schema(&helix_core::commit::load_commit(&lca)?.schema_hash)?;
            let ours = helix_core::commit::load_schema(&helix_core::commit::load_commit(&ours_hash)?.schema_hash)?;
            let theirs = helix_core::commit::load_schema(&helix_core::commit::load_commit(&theirs_hash)?.schema_hash)?;

            let result = helix_core::merge::merge_schemas(&base, &ours, &theirs);
            if !result.is_clean() {
                println!("{}", "Automatic merge failed; fix conflicts and commit manually:".red());
                for conflict in &result.conflicts {
                    println!("  {} {}", "CONFLICT".red().bold(), conflict.description);
                }
                bail!("merge aborted — {} conflict(s)", result.conflicts.len());
            }

            let message = format!("Merge branch '{name}' into {current_branch}");
            let hash = helix_core::commit::create_commit(&result.schema, &message)?;
            println!("Merge made ({}). {}", &hash[..8], message);
        }
        Commands::ExportMigration { from, to } => {
            let old = helix_core::commit::resolve_schema(&from)?;
            let new = helix_core::commit::resolve_schema(&to)?;
            let ops = helix_core::diff::diff_schemas(&old, &new);
            let statements = helix_core::migrate::to_sql(&ops);

            println!("-- Migration: {from} -> {to}");
            for statement in statements {
                println!("{statement}");
            }
        }
    }

    Ok(())
}

async fn live_schema() -> Result<helix_core::types::Schema> {
    let database_url = helix_core::database_url()?;
    let pool = helix_core::snapshot::connect(&database_url).await?;
    helix_core::snapshot::extract_schema(&pool).await
}

fn head_schema_or_empty() -> Result<helix_core::types::Schema> {
    match helix_core::commit::head_commit_hash()? {
        Some(hash) => Ok(helix_core::commit::load_schema(&helix_core::commit::load_commit(&hash)?.schema_hash)?),
        None => Ok(helix_core::types::Schema { tables: vec![] }),
    }
}

fn print_diff(ops: &[DiffOp]) {
    if ops.is_empty() {
        println!("No changes.");
        return;
    }
    for op in ops {
        match op {
            DiffOp::AddTable { name, .. } => println!("  {} table {name}", "+".green()),
            DiffOp::DropTable { name } => println!("  {} table {name}", "-".red()),
            DiffOp::AddColumn { table, column } => println!("  {} {table}.{}", "+".green(), column.name),
            DiffOp::DropColumn { table, column_name } => println!("  {} {table}.{column_name}", "-".red()),
            DiffOp::RenameColumn { table, from, to } => {
                println!("  {} {table}.{from} \u{21b7} {table}.{to}", "~".yellow())
            }
            DiffOp::ModifyColumn { table, column_name, .. } => println!("  {} {table}.{column_name}", "~".yellow()),
            DiffOp::AddConstraint { table, constraint } => {
                println!("  {} constraint {} on {table}", "+".green(), constraint.name)
            }
            DiffOp::DropConstraint { table, constraint_name } => {
                println!("  {} constraint {constraint_name} on {table}", "-".red())
            }
            DiffOp::AddIndex { table, index } => println!("  {} index {} on {table}", "+".green(), index.name),
            DiffOp::DropIndex { table, index_name } => println!("  {} index {index_name} on {table}", "-".red()),
        }
    }
}
