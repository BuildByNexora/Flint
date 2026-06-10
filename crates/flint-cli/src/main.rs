use std::path::PathBuf;

use clap::{Parser, Subcommand};
use flint_core::{Algorithm, Limiter, TopBy};

#[derive(Parser)]
#[command(name = "flint", about = "embedded persistent rate limiter")]
struct Cli {
    #[arg(long, global = true, default_value = ".flint")]
    data_dir: PathBuf,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Limit {
        #[command(subcommand)]
        command: LimitCommand,
    },
    Log {
        #[command(subcommand)]
        command: LogCommand,
    },
    Doctor,
}

#[derive(Subcommand)]
enum LimitCommand {
    Add {
        key: String,
        #[arg(long)]
        rate: u64,
        #[arg(long)]
        per: String,
        #[arg(long, default_value = "token_bucket")]
        algorithm: String,
    },
    List,
    Status {
        key: String,
    },
    Reset {
        key: String,
    },
    History {
        key: String,
    },
    Top {
        #[arg(long, default_value = "denied")]
        by: String,
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
}

#[derive(Subcommand)]
enum LogCommand {
    Compact,
}

fn main() {
    let cli = Cli::parse();
    if let Err(err) = run(cli) {
        eprintln!("flint: {err}");
        std::process::exit(1);
    }
}

fn run(cli: Cli) -> Result<(), Box<dyn std::error::Error>> {
    let limiter = Limiter::open(&cli.data_dir)?;
    match cli.command {
        Command::Limit { command } => match command {
            LimitCommand::Add {
                key,
                rate,
                per,
                algorithm,
            } => {
                limiter.limit(key, rate, per, Algorithm::parse(&algorithm)?)?;
                println!("configured");
            }
            LimitCommand::List => {
                println!("{}", serde_json::to_string_pretty(&limiter.list()?)?);
            }
            LimitCommand::Status { key } => {
                println!("{}", serde_json::to_string_pretty(&limiter.status(&key)?)?);
            }
            LimitCommand::Reset { key } => {
                limiter.reset(&key)?;
                println!("reset");
            }
            LimitCommand::History { key } => {
                println!("{}", serde_json::to_string_pretty(&limiter.history(&key)?)?);
            }
            LimitCommand::Top { by, limit } => {
                let by = match by.as_str() {
                    "allowed" => TopBy::Allowed,
                    "denied" => TopBy::Denied,
                    other => return Err(format!("unsupported top selector: {other}").into()),
                };
                println!(
                    "{}",
                    serde_json::to_string_pretty(&limiter.top(by, limit)?)?
                );
            }
        },
        Command::Log { command } => match command {
            LogCommand::Compact => {
                limiter.compact()?;
                println!("compacted");
            }
        },
        Command::Doctor => {
            println!("{}", serde_json::to_string_pretty(&limiter.doctor()?)?);
        }
    }
    Ok(())
}
