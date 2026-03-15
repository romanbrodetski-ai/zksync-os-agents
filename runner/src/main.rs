mod claude;
mod git;

use anyhow::{bail, Context, Result};
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

    fn display_name(&self) -> &'static str {
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
    /// Review a PR: cross-reference the diff against the knowledge base,
    /// confirm issues with tests, draft GitHub comments for user approval.
    ReviewPr {
        /// GitHub PR number (in matter-labs/zksync-os-server)
        pr_number: u64,
    },
    /// Advance the agent's server submodule to the latest main, then update
    /// knowledge and tests so they compile, pass, and reflect the new version.
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
        Command::ReviewPr { pr_number } => review_pr(agent, agent_path, submodule_path, pr_number),
        Command::UpdateMain => update_main(agent, repo_root, agent_path, submodule_path),
    }
}

fn review_pr(
    agent: Agent,
    agent_path: PathBuf,
    submodule_path: PathBuf,
    pr_number: u64,
) -> Result<()> {
    let base_sha = git::submodule_sha(&submodule_path)?;

    let session_name = format!("{} PR#{pr_number}", agent.display_name());

    let system_ctx = format!(
        "Mode: pr-review. \
         Agent: {agent}. \
         PR: #{pr_number} in matter-labs/zksync-os-server. \
         Server submodule SHA (base branch): {base_sha}.",
        agent = agent.display_name(),
    );

    let prompt = format!(
        "Run the {agent} agent on PR #{pr_number}.\n\
         \n\
         Follow the review process in AGENTS.md:\n\
         - Step 0: sync the submodule to the PR base branch (already at {base_sha}; \
           check it matches the PR base and update knowledge if the server has drifted).\n\
         - Step 1: fetch the diff with `gh pr diff {pr_number} -R matter-labs/zksync-os-server`.\n\
         - Steps 2–3: cross-reference knowledge, confirm issues with tests.\n\
         - Step 4: present all draft comments here before posting anything to GitHub.\n\
         - Steps 5–6: post confirmed comments, update knowledge and tests.",
        agent = agent.display_name(),
    );

    claude::exec_claude(&agent_path, &session_name, &system_ctx, &prompt);
}

fn update_main(
    agent: Agent,
    repo_root: PathBuf,
    agent_path: PathBuf,
    _submodule_path: PathBuf,
) -> Result<()> {
    println!(
        "Updating {}'s server submodule to latest main…",
        agent.display_name()
    );

    let (old_sha, new_sha) = git::update_submodule_to_main(&repo_root, agent.dir())?;

    if old_sha == new_sha {
        println!("Submodule already at latest main ({new_sha}). Nothing to do.");
        return Ok(());
    }

    println!("Submodule updated: {old_sha} → {new_sha}");

    let session_name = format!("{} update-main", agent.display_name());

    let system_ctx = format!(
        "Mode: update-main. \
         Agent: {agent}. \
         Server submodule updated from {old_sha} to {new_sha}.",
        agent = agent.display_name(),
    );

    let prompt = format!(
        "The {agent} agent's server submodule has been updated from {old_sha} to {new_sha}.\n\
         \n\
         Your job:\n\
         1. Read what changed: `git -C zksync-os-server log --oneline {old_sha}..{new_sha}`\n\
            and review the relevant diffs.\n\
         2. Update knowledge files so they reflect the new server version.\n\
         3. Update tests so they compile and pass against the new server code.\n\
         4. Run the agent's test suite (command in AGENTS.md) to confirm.\n\
         5. Commit knowledge/ and the bumped submodule pointer atomically.\n\
         \n\
         Do not post GitHub PR comments. If you find a breaking change that needs \
         human judgment, stop and escalate it here in the chat.",
        agent = agent.display_name(),
    );

    claude::exec_claude(&agent_path, &session_name, &system_ctx, &prompt);
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
            None => bail!(
                "could not find a git repo root (no .git found above {})",
                cwd.display()
            ),
        }
    }
}
