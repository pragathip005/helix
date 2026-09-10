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
        Commands::Commit { message: _ } => {
            todo!("Story 2.4")
        }
        Commands::Log => {
            todo!("Story 2.5")
        }
        Commands::Status => {
            todo!("Story 1.6")
        }
        Commands::Diff { target: _, target2: _ } => {
            todo!("Story 3.8")
        }
        Commands::Branch { name: _ } => {
            todo!("Story 4.1 / 4.2")
        }
        Commands::Checkout { name: _ } => {
            todo!("Story 4.3")
        }
        Commands::Merge { name: _ } => {
            todo!("Story 5.5")
        }
        Commands::ExportMigration { from: _, to: _ } => {
            todo!("Story 6.3")
        }
    }

    Ok(())
}
