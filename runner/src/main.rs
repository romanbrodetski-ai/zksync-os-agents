mod claude;
mod codex;
mod gh;
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

    #[arg(long)]
    model: Option<String>,

    #[command(subcommand)]
    command: Command,
}

#[derive(ValueEnum, Clone, Debug)]
enum Agent {
    BlockRebuildMaintainer,
    L1Settle,
    PipelineCorrectness,
    SepoliaDeploy,
    TxInclusionServerRestart,
}

impl Agent {
    fn dir(&self) -> &'static str {
        match self {
            Agent::BlockRebuildMaintainer => "block-rebuild-maintainer",
            Agent::L1Settle => "l1-settle",
            Agent::PipelineCorrectness => "pipeline-correctness",
            Agent::SepoliaDeploy => "sepolia-deploy",
            Agent::TxInclusionServerRestart => "tx-inclusion-server-restart",
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
        /// Full GitHub PR URL (e.g. https://github.com/matter-labs/zksync-os-server/pull/123)
        pr_url: String,
    },
    /// Update the agent to a specific server commit: branch, tag, or SHA.
    Update {
        /// Git ref to update to (branch, tag, or full SHA)
        target: String,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    let repo_root = find_repo_root()?;
    let agent_path = repo_root.join(cli.agent.dir());
    let submodule_path = agent_path.join("zksync-os-server");
    let bot_name = cli.agent.dir();

    git::ensure_submodule_initialized(&repo_root, &submodule_path)?;
    git::check_submodule_clean(&submodule_path)
        .context("submodule must be clean before running an agent")?;

    let current = git::current_sha(&submodule_path)?;

    match cli.command {
        Command::ReviewPr { pr_url } => {
            let (server_repo, pr_number) = parse_pr_url(&pr_url)?;
            let (base, head) = git::pr_shas(&server_repo, pr_number)?;
            if current != base {
                anyhow::bail!(
                    "submodule is at {current} but PR #{pr_number} base is {base} — \
                     run update-main first"
                );
            }
            git::print_diff_summary(&submodule_path, &base, &head)?;

            let start = std::time::Instant::now();
            run_ai(&cli.ai, &agent_path, &prompts::agent_prompt(&base, &head), cli.model.as_deref())?;
            let duration = start.elapsed();

            let server_pr = gh::find_server_pr_url(&server_repo, &head);
            if let Some(agent_pr) = gh::latest_open_pr_url(&agent_path)? {
                gh::prepend_pr_metadata(&agent_pr, bot_name, cli.model.as_deref(), duration, server_pr)?;
            }
        }
        Command::Update { target } => {
            let new = git::resolve_ref(&submodule_path, &target)?;
            if current == new {
                println!("Already at {target} ({new}). Nothing to do.");
                return Ok(());
            }
            git::print_diff_summary(&submodule_path, &current, &new)?;

            let start = std::time::Instant::now();
            run_ai(&cli.ai, &agent_path, &prompts::agent_prompt(&current, &new), cli.model.as_deref())?;
            let duration = start.elapsed();

            let server_pr = gh::find_server_pr_url("matter-labs/zksync-os-server", &new);
            if let Some(agent_pr) = gh::latest_open_pr_url(&agent_path)? {
                gh::prepend_pr_metadata(&agent_pr, bot_name, cli.model.as_deref(), duration, server_pr)?;
            }
        }
    }

    Ok(())
}

fn run_ai(ai: &Ai, agent_dir: &std::path::Path, prompt: &str, model: Option<&str>) -> Result<()> {
    match ai {
        Ai::Claude => claude::run(agent_dir, prompt, model),
        Ai::Codex => codex::run(agent_dir, prompt, model),
    }
}

/// Parses a GitHub PR URL into (repo, pr_number).
/// Accepts https://github.com/{owner}/{repo}/pull/{number}.
fn parse_pr_url(url: &str) -> Result<(String, u64)> {
    let path = url
        .strip_prefix("https://github.com/")
        .with_context(|| format!("PR URL must start with https://github.com/ — got: {url}"))?;

    let parts: Vec<&str> = path.splitn(4, '/').collect();
    if parts.len() < 4 || parts[2] != "pull" {
        anyhow::bail!("invalid GitHub PR URL: {url}");
    }

    let repo = format!("{}/{}", parts[0], parts[1]);
    let number: u64 = parts[3]
        .parse()
        .with_context(|| format!("invalid PR number in URL: {url}"))?;
    Ok((repo, number))
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
