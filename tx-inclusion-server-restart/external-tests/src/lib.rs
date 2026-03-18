use alloy::eips::BlockId;
use alloy::network::EthereumWallet;
use alloy::primitives::{Address, U256};
use alloy::providers::{Provider, ProviderBuilder};
use alloy::rpc::types::TransactionRequest;
use alloy::network::TransactionBuilder;
use alloy::signers::local::PrivateKeySigner;
use anyhow::{Context, anyhow};
use reqwest::StatusCode;
use serde_json::json;
use std::fs::File;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::process::{Child, Command};
use url::Url;

const RICH_PRIVATE_KEY: &str =
    "0x7726827caac94a7f9e1b160f7ea819f172f7b6f9d2a97f992c38edeab82d4110";

pub struct TestEnvironment {
    pub artifacts_dir: PathBuf,
    pub anvil_log_path: PathBuf,
    pub server_log_path: PathBuf,
    anvil: ChildProcess,
    server: ChildProcess,
    pub rpc_url: String,
    pub status_url: String,
    pub logs_dir: PathBuf,
}

pub struct TxLifecycleReport {
    pub tx_hash: String,
    pub block_number: u64,
    pub inclusion_latency: Duration,
    pub safe_latency: Duration,
    pub finalized_latency: Duration,
}

pub struct TxInclusionReport {
    pub tx_hash: String,
    pub block_number: u64,
    pub inclusion_latency: Duration,
}

struct ChildProcess {
    child: Child,
}

impl TestEnvironment {
    pub async fn start() -> anyhow::Result<Self> {
        Self::start_with_block_time(None).await
    }

    pub async fn start_with_block_time(block_time: Option<Duration>) -> anyhow::Result<Self> {
        let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("crate has parent directory")
            .join("zksync-os-server");
        let artifacts_dir = create_artifacts_dir()?;
        let logs_dir = artifacts_dir.join("logs");
        std::fs::create_dir_all(&logs_dir)?;

        let anvil_port = pick_unused_port()?;
        let rpc_port = pick_unused_port()?;
        let status_port = pick_unused_port()?;
        let prometheus_port = pick_unused_port()?;

        let anvil_log = logs_dir.join("anvil.log");
        let mut anvil_cmd = Command::new("anvil");
        anvil_cmd
            .arg("--load-state")
            .arg(repo_root.join("local-chains/v30.2/l1-state.json"))
            .arg("--port")
            .arg(anvil_port.to_string());
        let anvil = spawn_logged("anvil", &mut anvil_cmd, &anvil_log)
            .await
            .context("failed to spawn anvil")?;
        let anvil_url = format!("http://127.0.0.1:{anvil_port}");
        wait_for_rpc(&anvil_url, Duration::from_secs(20)).await?;

        let server_log = logs_dir.join("server.log");
        let rocks_db_path = artifacts_dir.join("db");
        let config_path = repo_root.join("local-chains/v30.2/default/config.yaml");
        let mut server_cmd = Command::new("cargo");
        server_cmd
            .arg("run")
            .arg("--manifest-path")
            .arg(repo_root.join("Cargo.toml"))
            .arg("--bin")
            .arg("zksync-os-server")
            .arg("--")
            .arg("--config")
            .arg(config_path)
            .current_dir(&repo_root)
            .env("general_l1_rpc_url", &anvil_url)
            .env("general_rocks_db_path", &rocks_db_path)
            .env("rpc_address", format!("127.0.0.1:{rpc_port}"))
            .env("status_server_enabled", "true")
            .env("status_server_address", format!("127.0.0.1:{status_port}"))
            .env("observability_prometheus_port", prometheus_port.to_string())
            .env("RUST_LOG", "info");
        if let Some(block_time) = block_time {
            server_cmd.env("sequencer_block_time", format_duration(block_time));
        }
        let server = spawn_logged("server", &mut server_cmd, &server_log)
            .await
            .context("failed to spawn zksync-os-server")?;

        let rpc_url = format!("http://127.0.0.1:{rpc_port}");
        let status_url = format!("http://127.0.0.1:{status_port}/status/health");
        wait_for_health(&status_url, Duration::from_secs(120)).await?;

        let provider = default_provider(rpc_url.clone())?;
        let rich_address = default_signer()?.address();
        wait_for_balance(&provider, rich_address, Duration::from_secs(60)).await?;

        Ok(Self {
            artifacts_dir,
            anvil_log_path: anvil_log,
            server_log_path: server_log,
            anvil,
            server,
            rpc_url,
            status_url,
            logs_dir,
        })
    }

    pub fn provider(&self) -> anyhow::Result<impl Provider> {
        default_provider(self.rpc_url.clone())
    }

    pub async fn run_basic_transfer(&self) -> anyhow::Result<TxLifecycleReport> {
        let provider = self.provider()?;
        let submitted_at = Instant::now();
        let pending = provider
            .send_transaction(
                TransactionRequest::default()
                    .with_to(Address::repeat_byte(0x11))
                    .with_value(U256::from(1_u64)),
            )
            .await
            .context("failed to submit transaction")?;
        let tx_hash = pending.tx_hash().to_string();

        let receipt = pending
            .get_receipt()
            .await
            .context("transaction was not included in time")?;
        let inclusion_latency = submitted_at.elapsed();
        let block_number = receipt
            .block_number
            .ok_or_else(|| anyhow!("receipt is missing block number"))?;

        wait_for_block_tag(&provider, block_number, BlockId::safe(), Duration::from_secs(180))
            .await?;
        let safe_latency = submitted_at.elapsed();

        wait_for_block_tag(
            &provider,
            block_number,
            BlockId::finalized(),
            Duration::from_secs(180),
        )
        .await?;
        let finalized_latency = submitted_at.elapsed();

        Ok(TxLifecycleReport {
            tx_hash,
            block_number,
            inclusion_latency,
            safe_latency,
            finalized_latency,
        })
    }

    pub async fn run_basic_transfer_inclusion_only(&self) -> anyhow::Result<TxInclusionReport> {
        let provider = self.provider()?;
        let submitted_at = Instant::now();
        let pending = provider
            .send_transaction(
                TransactionRequest::default()
                    .with_to(Address::repeat_byte(0x11))
                    .with_value(U256::from(1_u64)),
            )
            .await
            .context("failed to submit transaction")?;
        let tx_hash = pending.tx_hash().to_string();

        let receipt = pending
            .get_receipt()
            .await
            .context("transaction was not included in time")?;
        let inclusion_latency = submitted_at.elapsed();
        let block_number = receipt
            .block_number
            .ok_or_else(|| anyhow!("receipt is missing block number"))?;

        Ok(TxInclusionReport {
            tx_hash,
            block_number,
            inclusion_latency,
        })
    }
}

impl Drop for TestEnvironment {
    fn drop(&mut self) {
        self.server.kill();
        self.anvil.kill();
    }
}

impl ChildProcess {
    fn kill(&mut self) {
        let _ = self.child.start_kill();
    }
}

impl Drop for ChildProcess {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
    }
}

async fn spawn_logged(
    name: &'static str,
    command: &mut Command,
    log_path: &Path,
) -> anyhow::Result<ChildProcess> {
    let stdout = File::create(log_path)
        .with_context(|| format!("failed to create stdout log for {name}"))?;
    let stderr = stdout
        .try_clone()
        .with_context(|| format!("failed to clone stderr log for {name}"))?;
    let child = command
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr))
        .spawn()
        .with_context(|| format!("failed to spawn {name}"))?;
    Ok(ChildProcess { child })
}

fn default_signer() -> anyhow::Result<PrivateKeySigner> {
    RICH_PRIVATE_KEY.parse().context("failed to parse rich private key")
}

fn default_provider(rpc_url: String) -> anyhow::Result<impl Provider + Clone> {
    let signer = default_signer()?;
    let wallet = EthereumWallet::from(signer);
    let url = Url::parse(&rpc_url)?;
    Ok(ProviderBuilder::new().wallet(wallet).connect_http(url))
}

async fn wait_for_rpc(rpc_url: &str, timeout: Duration) -> anyhow::Result<()> {
    let client = reqwest::Client::new();
    let started = Instant::now();
    loop {
        let response = client
            .post(rpc_url)
            .json(&json!({
                "jsonrpc": "2.0",
                "method": "eth_chainId",
                "params": [],
                "id": 1
            }))
            .send()
            .await;
        if let Ok(response) = response
            && response.status().is_success()
        {
            return Ok(());
        }
        if started.elapsed() >= timeout {
            anyhow::bail!("timed out waiting for rpc at {rpc_url}");
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

async fn wait_for_health(status_url: &str, timeout: Duration) -> anyhow::Result<()> {
    let client = reqwest::Client::new();
    let started = Instant::now();
    loop {
        let response = client.get(status_url).send().await;
        if let Ok(response) = response
            && response.status() == StatusCode::OK
        {
            return Ok(());
        }
        if started.elapsed() >= timeout {
            anyhow::bail!("timed out waiting for status endpoint at {status_url}");
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

async fn wait_for_balance<P: Provider>(
    provider: &P,
    address: Address,
    timeout: Duration,
) -> anyhow::Result<()> {
    let started = Instant::now();
    loop {
        let balance = provider.get_balance(address).await?;
        if balance > U256::ZERO {
            return Ok(());
        }
        if started.elapsed() >= timeout {
            anyhow::bail!("timed out waiting for funded test wallet");
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

async fn wait_for_block_tag<P: Provider>(
    provider: &P,
    expected_block: u64,
    block_id: BlockId,
    timeout: Duration,
) -> anyhow::Result<()> {
    let started = Instant::now();
    loop {
        if let Some(block_number) = provider.get_block_number_by_id(block_id).await?
            && block_number >= expected_block
        {
            return Ok(());
        }
        if started.elapsed() >= timeout {
            anyhow::bail!("timed out waiting for block {expected_block} to reach {block_id:?}");
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

fn pick_unused_port() -> anyhow::Result<u16> {
    let listener = TcpListener::bind("127.0.0.1:0").context("failed to bind random port")?;
    let port = listener.local_addr()?.port();
    drop(listener);
    Ok(port)
}

fn create_artifacts_dir() -> anyhow::Result<PathBuf> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(".artifacts");
    std::fs::create_dir_all(&root)?;
    let timestamp_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time went backwards")
        .as_millis();
    let path = root.join(format!("run-{timestamp_ms}-{}", std::process::id()));
    std::fs::create_dir_all(&path)?;
    Ok(path)
}

fn format_duration(duration: Duration) -> String {
    if duration.as_millis() < 1000 {
        format!("{}ms", duration.as_millis())
    } else {
        format!("{}s", duration.as_secs())
    }
}
