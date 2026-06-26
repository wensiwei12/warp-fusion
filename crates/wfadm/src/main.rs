mod init;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "wfadm",
    version,
    about = "WarpFusion admin CLI — project management for wf-rules",
    propagate_version = true
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Create a new wf-rules project
    Init {
        /// Project name
        #[arg(long)]
        name: Option<String>,
        /// Project directory
        #[arg(long)]
        dir: Option<String>,
    },
    /// Check project integrity
    Check,
    /// Validate sink configuration
    Sink,
    /// Self-update binary
    #[command(name = "self-update")]
    SelfUpdate,
}

fn main() {
    let cli = Cli::parse();

    let result = match cli.command {
        Commands::Init { name, dir } => {
            let project_dir = dir.unwrap_or_else(|| ".".to_string());
            let project_name = name.unwrap_or_else(|| {
                std::path::Path::new(&project_dir)
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| "wf-rules".to_string())
            });
            init::init_project(&project_dir, &project_name)
        }
        Commands::Check => {
            eprintln!("TODO: check");
            Ok(())
        }
        Commands::Sink => {
            eprintln!("TODO: sink");
            Ok(())
        }
        Commands::SelfUpdate => {
            eprintln!("TODO: self-update");
            Ok(())
        }
    };

    if let Err(e) = result {
        eprintln!("Error: {e}");
        std::process::exit(1);
    }
}
