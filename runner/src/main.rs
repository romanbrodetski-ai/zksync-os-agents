mod claude;
mod git;
mod prompts;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use std::path::{Path, PathBuf};

#[derive(Parser)]
#[command(name = "runner", about = "ZKsync OS agent runner")]
struct Cli {
    /// Agent to run
    #[arg(long, short)]
    agent: Agent,

    /// AI backend
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
    /// Review a PR: sync to base, cross-reference knowledge, confirm with tests, output findings.
    ReviewPr {
        /// GitHub PR number (in matter-labs/zksync-os-server)
        pr_number: u64,
    },
    /// Advance the agent's server submodule to latest main, sync knowledge and tests.
    UpdateMain,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.ai {
        Ai::Codex => todo!("Codex support not yet implemented"),
        Ai::Claude => run_claude(cli.agent, cli.command),
    }
}

fn run_claude(agent: Agent, command: Command) -> Result<()> {
    let repo_root = find_repo_root()?;
    let agent_path = repo_root.join(agent.dir());
    let submodule_path = agent_path.join("zksync-os-server");

    git::check_submodule_clean(&submodule_path)
        .context("submodule must be clean before running an agent")?;

    match command {
        Command::ReviewPr { pr_number } => review_pr(agent_path, submodule_path, pr_number),
        Command::UpdateMain => update_main(agent, repo_root, agent_path, submodule_path),
    }
}

fn review_pr(agent_path: PathBuf, submodule_path: PathBuf, pr_number: u64) -> Result<()> {
    let (base_sha, head_sha) = git::pr_shas(pr_number)?;
    let current_sha = git::submodule_sha(&submodule_path)?;

    // If submodule is behind the PR base, sync knowledge/tests to base first.
    if current_sha != base_sha {
        println!("Syncing to PR base: {current_sha} → {base_sha}");
        claude::run_claude(
            &agent_path,
            &format!("PR#{pr_number} sync-base"),
            prompts::SYSTEM_CTX,
            &prompts::agent_prompt(&current_sha, &base_sha),
        )?;
    } else {
        println!("Submodule already at PR base {base_sha}. Skipping sync.");
    }

    // Now examine the PR diff: base → head.
    println!("Reviewing PR#{pr_number}: {base_sha} → {head_sha}");
    claude::exec_claude(
        &agent_path,
        &format!("PR#{pr_number} review"),
        prompts::SYSTEM_CTX,
        &prompts::agent_prompt(&base_sha, &head_sha),
    );
}

fn update_main(
    agent: Agent,
    repo_root: PathBuf,
    agent_path: PathBuf,
    _submodule_path: PathBuf,
) -> Result<()> {
    let (old_sha, new_sha) = git::update_submodule_to_main(&repo_root, agent.dir())?;

    if old_sha == new_sha {
        println!("Submodule already at latest main ({new_sha}). Nothing to do.");
        return Ok(());
    }

    // Restore submodule to old SHA — agent checks out new SHA itself as part of its work.
    let submodule_path = repo_root.join(agent.dir()).join("zksync-os-server");
    git::checkout_submodule_sha(&submodule_path, &old_sha)?;

    println!("Updating: {old_sha} → {new_sha}");
    claude::exec_claude(
        &agent_path,
        "update-main",
        prompts::SYSTEM_CTX,
        &prompts::agent_prompt(&old_sha, &new_sha),
    );
}

/// Walks up from CWD until it finds a directory containing `.git`.
fn find_repo_root() -> Result<PathBuf> {
    let cwd = std::env::current_dir().context("failed to get current directory")?;
    let mut dir: &Path = &cwd;
    loop {
        if dir.join(".git").exists() {
            return Ok(dir.to_path_buf());
        }
        match dir.parent() {
            Some(parent) => dir = parent,
            None => anyhow::bail!(
                "could not find a git repo root (no .git found above {})",
                cwd.display()
            ),
        }
    }
}
