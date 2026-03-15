use clap::{Parser, Subcommand, ValueEnum};

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
    /// Directory name within the repo root.
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
    /// Review a PR: check diff against knowledge base, confirm issues with tests, draft comments.
    ReviewPr {
        /// GitHub PR number to review
        pr_number: u64,
    },
    /// Update agent's server submodule to latest main, then sync knowledge and tests.
    UpdateMain,
}

fn main() {
    let cli = Cli::parse();

    match cli.ai {
        Ai::Codex => todo!("Codex support not yet implemented"),
        Ai::Claude => run_claude(cli.agent, cli.command),
    }
}

fn run_claude(agent: Agent, command: Command) {
    let _agent_dir = agent.dir();
    match command {
        Command::ReviewPr { pr_number: _ } => todo!(),
        Command::UpdateMain => todo!(),
    }
}
