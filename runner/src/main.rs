mod claude;
mod git;
mod prompts;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "runner", about = "ZKsync OS agent runner")]
struct Cli {
    #[arg(long, short)]
    agent: Agent,

    #[arg(long, default_value = "claude")]
    ai: Ai,

    #[command(subcommand)]
    command: Command,
}

#[derive(ValueEnum, Clone, Debug)]
enum Agent {
    BlockRebuildMaintainer,
    L1Settle,
    PipelineCorrectness,
    SepoliaDeploy,
}

impl Agent {
    fn dir(&self) -> &'static str {
        match self {
            Agent::BlockRebuildMaintainer => "block-rebuild-maintainer",
            Agent::L1Settle => "l1-settle",
            Agent::PipelineCorrectness => "pipeline-correctness",
            Agent::SepoliaDeploy => "sepolia-deploy",
        }
    }
}

#[derive(ValueEnum, Clone, Debug)]
enum Ai {
    Claude,
    Codex,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Review a PR. The PR's base branch must already be the agent's current submodule SHA.
    ReviewPr {
        /// GitHub PR number (in matter-labs/zksync-os-server)
        pr_number: u64,
    },
    /// Update the agent to a specific server commit: branch, tag, or SHA.
    Update {
        /// Git ref to update to (branch, tag, or full SHA)
        target: String,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.ai {
        Ai::Codex => todo!("Codex support not yet implemented"),
        Ai::Claude => {}
    }

    let repo_root = find_repo_root()?;
    let agent_path = repo_root.join(cli.agent.dir());
    let submodule_path = agent_path.join("zksync-os-server");

    git::check_submodule_clean(&submodule_path)
        .context("submodule must be clean before running an agent")?;

    let current = git::current_sha(&submodule_path)?;

    match cli.command {
        Command::ReviewPr { pr_number } => {
            let (base, head) = git::pr_shas(pr_number)?;
            if current != base {
                anyhow::bail!(
                    "submodule is at {current} but PR #{pr_number} base is {base} — \
                     run update-main first"
                );
            }
            git::print_diff_summary(&submodule_path, &base, &head)?;
            claude::exec(
                &agent_path,
                prompts::SYSTEM_CTX,
                &prompts::agent_prompt(&base, &head),
            );
        }
        Command::Update { target } => {
            let new = git::resolve_ref(&submodule_path, &target)?;
            if current == new {
                println!("Already at {target} ({new}). Nothing to do.");
                return Ok(());
            }
            git::print_diff_summary(&submodule_path, &current, &new)?;
            claude::exec(
                &agent_path,
                prompts::SYSTEM_CTX,
                &prompts::agent_prompt(&current, &new),
            );
        }
    }
}

/// Walks up from CWD until it finds a directory containing `.git`.
fn find_repo_root() -> Result<PathBuf> {
    let cwd = std::env::current_dir().context("failed to get current directory")?;
    for dir in cwd.ancestors() {
        if dir.join(".git").exists() {
            return Ok(dir.to_path_buf());
        }
    }
    anyhow::bail!("could not find a git repo root (no .git found above {})", cwd.display())
}
