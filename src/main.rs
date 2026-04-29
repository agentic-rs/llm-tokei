use std::{path::PathBuf, process::ExitCode};

use anyhow::Result;
use clap::{Parser, ValueEnum};
use llm_tokei::{ProviderKind, output, summarize};

#[derive(Debug, Parser)]
#[command(
    version,
    about = "Summarize token usage from LLM coding agent sessions"
)]
struct Cli {
    #[arg(long, value_enum, default_value_t = ProviderArg::OpenCode)]
    provider: ProviderArg,

    #[arg(long = "path", value_name = "PATH")]
    paths: Vec<PathBuf>,

    #[arg(long, value_enum, default_value_t = Format::Table)]
    format: Format,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum ProviderArg {
    OpenCode,
}

impl From<ProviderArg> for ProviderKind {
    fn from(value: ProviderArg) -> Self {
        match value {
            ProviderArg::OpenCode => Self::OpenCode,
        }
    }
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum Format {
    Table,
    Json,
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error:?}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    let summary = summarize(cli.provider.into(), cli.paths)?;
    let rendered = match cli.format {
        Format::Table => output::table(&summary),
        Format::Json => output::json(&summary)?,
    };

    println!("{rendered}");
    Ok(())
}
