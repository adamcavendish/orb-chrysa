use clap::Parser;
use thiserror::Error;

mod air_gapped;
mod mirror;
mod registry;

#[derive(Parser)]
#[command(name = "orb-chrysa-cli", about = "orb-chrysa registry CLI")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(clap::Subcommand)]
enum Commands {
    Status,
    /// Air-gapped Kubernetes bootstrap helpers
    #[command(name = "air-gapped")]
    AirGapped {
        #[command(subcommand)]
        command: air_gapped::AirGappedCommands,
    },
    /// Mirror images between registries
    Mirror {
        /// Source image reference (e.g., docker.io/library/alpine:3.20)
        source: String,
        /// Destination image reference (e.g., localhost:5050/mirror/alpine:3.20)
        destination: String,
        /// Maximum parallel blob transfers
        #[arg(short, long, default_value = "8")]
        concurrency: usize,
        /// Mirror all tags from source repository
        #[arg(long)]
        all: bool,
        /// Use HTTP instead of HTTPS
        #[arg(long)]
        plain_http: bool,
    },
}

#[tokio::main]
async fn main() -> Result<(), CliError> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Status => {
            println!("orb-chrysa CLI (not yet implemented)");
        }
        Commands::AirGapped { command } => {
            air_gapped::run(command)?;
        }
        Commands::Mirror {
            source,
            destination,
            concurrency,
            all,
            plain_http,
        } => {
            let src = registry::ImageRef::parse(&source, plain_http)?;
            let dst = registry::ImageRef::parse(&destination, plain_http)?;

            eprintln!("Mirroring {} -> {}", src.display(), dst.display());

            mirror::mirror(
                src,
                dst,
                mirror::MirrorOptions {
                    concurrency,
                    all_tags: all,
                },
            )
            .await?;
        }
    }

    Ok(())
}

#[derive(Debug, Error)]
enum CliError {
    #[error("{0}")]
    Registry(#[from] registry::RegistryError),
    #[error("{0}")]
    AirGapped(#[from] air_gapped::AirGappedError),
}
