use alloy::eips::BlockId;
use alloy::network::EthereumWallet;
use alloy::primitives::{Address, B256, U256};
use alloy::providers::{Provider, ProviderBuilder};
use alloy::network::TransactionBuilder;
use alloy::rpc::types::TransactionRequest;
use alloy::signers::local::PrivateKeySigner;
use anyhow::{Context, anyhow};
use reqwest::StatusCode;
use serde_json::json;
use k256::ecdsa::SigningKey;
use k256::elliptic_curve::rand_core::OsRng;
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
    repo_root: PathBuf,
    server_binary: PathBuf,
    pub artifacts_dir: PathBuf,
    pub anvil_log_path: PathBuf,
    pub server_log_path: PathBuf,
    anvil: ChildProcess,
    server: Option<ChildProcess>,
    pub rpc_url: String,
    pub status_url: String,
    pub logs_dir: PathBuf,
    l1_rpc_url: String,
    main_node_record: String,
    main_node_config: NodeProcessConfig,
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

pub struct ExternalNode {
    repo_root: PathBuf,
    server_binary: PathBuf,
    pub rpc_url: String,
    pub status_url: String,
    pub server_log_path: PathBuf,
    pub artifacts_dir: PathBuf,
    server: Option<ChildProcess>,
    node_config: NodeProcessConfig,
}

struct ChildProcess {
    child: Child,
}

#[derive(Clone)]
struct NodeProcessConfig {
    rocks_db_path: PathBuf,
    rpc_port: u16,
    status_port: u16,
    prometheus_port: u16,
    prover_api_port: u16,
    proof_storage_path: PathBuf,
    network_port: u16,
    network_secret_key: B256,
    block_time: Option<Duration>,
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
        let server_binary = ensure_server_binary(&repo_root).await?;
        let artifacts_dir = create_artifacts_dir()?;
        let logs_dir = artifacts_dir.join("logs");
        std::fs::create_dir_all(&logs_dir)?;

        let anvil_port = pick_unused_port()?;
        let rpc_port = pick_unused_port()?;
        let status_port = pick_unused_port()?;
        let prometheus_port = pick_unused_port()?;
        let prover_api_port = pick_unused_port()?;
        let network_port = pick_unused_port()?;
        let network_secret_key = random_network_secret_key();
        let main_node_record = enode_from_secret_key(&network_secret_key, network_port);

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
        let main_node_config = NodeProcessConfig {
            rocks_db_path: artifacts_dir.join("db"),
            rpc_port,
            status_port,
            prometheus_port,
            prover_api_port,
            proof_storage_path: artifacts_dir.join("fri_proofs"),
            network_port,
            network_secret_key,
            block_time,
        };
        let server = Some(
            spawn_main_node(
                &repo_root,
                &server_binary,
                &anvil_url,
                &main_node_config,
                &server_log,
            )
            .await
            .context("failed to spawn zksync-os-server")?,
        );

        let rpc_url = main_node_rpc_url(&main_node_config);
        let status_url = main_node_status_url(&main_node_config);

        let provider = default_provider(rpc_url.clone())?;
        let rich_address = default_signer()?.address();
        wait_for_balance(&provider, rich_address, Duration::from_secs(60)).await?;

        Ok(Self {
            repo_root,
            server_binary,
            artifacts_dir,
            anvil_log_path: anvil_log,
            server_log_path: server_log,
            anvil,
            server,
            rpc_url,
            status_url,
            logs_dir,
            l1_rpc_url: anvil_url,
            main_node_record,
            main_node_config,
        })
    }

    pub fn provider(&self) -> anyhow::Result<impl Provider> {
        default_provider(self.rpc_url.clone())
    }

    pub fn provider_for_signer(
        &self,
        signer: PrivateKeySigner,
    ) -> anyhow::Result<impl Provider + Clone> {
        provider_for_rpc_and_signer(self.rpc_url.clone(), signer)
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

    pub async fn fund_account(&self, address: Address, amount: U256) -> anyhow::Result<()> {
        let provider = self.provider()?;
        provider
            .send_transaction(
                TransactionRequest::default()
                    .with_to(address)
                    .with_value(amount),
            )
            .await
            .context("failed to submit funding transaction")?
            .get_receipt()
            .await
            .context("funding transaction was not included in time")?;
        Ok(())
    }

    pub async fn restart_main_node(&mut self) -> anyhow::Result<()> {
        if let Some(server) = self.server.as_mut() {
            server.kill().await;
        }
        let server = spawn_main_node(
            &self.repo_root,
            &self.server_binary,
            &self.l1_rpc_url,
            &self.main_node_config,
            &self.server_log_path,
        )
        .await
        .context("failed to restart zksync-os-server")?;
        self.server = Some(server);
        wait_for_health(&self.status_url, Duration::from_secs(120)).await?;

        Ok(())
    }

    pub async fn get_transaction_receipt_json(
        &self,
        tx_hash: B256,
    ) -> anyhow::Result<Option<serde_json::Value>> {
        let client = reqwest::Client::new();
        let response = client
            .post(&self.rpc_url)
            .json(&json!({
                "jsonrpc": "2.0",
                "method": "eth_getTransactionReceipt",
                "params": [format!("{tx_hash:#x}")],
                "id": 1
            }))
            .send()
            .await
            .context("failed to call eth_getTransactionReceipt")?;
        let response: serde_json::Value = response.json().await?;
        Ok(response["result"].clone().as_object().map(|_| response["result"].clone()))
    }

    pub async fn send_basic_transfer(&self) -> anyhow::Result<B256> {
        let provider = self.provider()?;
        let pending = provider
            .send_transaction(
                TransactionRequest::default()
                    .with_to(Address::repeat_byte(0x11))
                    .with_value(U256::from(1_u64)),
            )
            .await
            .context("failed to submit transaction")?;
        Ok(*pending.tx_hash())
    }

    pub async fn wait_for_receipt_json(&self, tx_hash: B256) -> anyhow::Result<serde_json::Value> {
        let started = Instant::now();
        loop {
            if let Some(receipt) = self.get_transaction_receipt_json(tx_hash).await? {
                return Ok(receipt);
            }
            if started.elapsed() >= Duration::from_secs(120) {
                anyhow::bail!("timed out waiting for receipt for {tx_hash:#x}");
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    pub async fn recipient_balance(&self, address: Address) -> anyhow::Result<U256> {
        let provider = self.provider()?;
        Ok(provider.get_balance(address).await?)
    }

    pub async fn wait_for_safe(&self, block_number: u64, timeout: Duration) -> anyhow::Result<()> {
        let provider = self.provider()?;
        wait_for_block_tag(&provider, block_number, BlockId::safe(), timeout).await
    }

    pub async fn wait_for_finalized(
        &self,
        block_number: u64,
        timeout: Duration,
    ) -> anyhow::Result<()> {
        let provider = self.provider()?;
        wait_for_block_tag(&provider, block_number, BlockId::finalized(), timeout).await
    }

    pub async fn launch_external_node(&self) -> anyhow::Result<ExternalNode> {
        let rpc_port = pick_unused_port()?;
        let status_port = pick_unused_port()?;
        let prometheus_port = pick_unused_port()?;
        let prover_api_port = pick_unused_port()?;
        let network_port = pick_unused_port()?;
        let network_secret_key = random_network_secret_key();

        let artifacts_dir = self.artifacts_dir.join(format!("external-node-{rpc_port}"));
        let logs_dir = artifacts_dir.join("logs");
        std::fs::create_dir_all(&logs_dir)?;

        let server_log = logs_dir.join("server.log");
        let node_config = NodeProcessConfig {
            rocks_db_path: artifacts_dir.join("db"),
            rpc_port,
            status_port,
            prometheus_port,
            prover_api_port,
            proof_storage_path: artifacts_dir.join("fri_proofs"),
            network_port,
            network_secret_key,
            block_time: None,
        };
        let server = Some(
            spawn_external_node(
                &self.repo_root,
                &self.server_binary,
                &self.l1_rpc_url,
                &self.rpc_url,
                &self.main_node_record,
                &node_config,
                &server_log,
            )
            .await
            .context("failed to spawn external node")?,
        );

        let rpc_url = main_node_rpc_url(&node_config);
        let status_url = main_node_status_url(&node_config);
        let provider = default_provider(rpc_url.clone())?;
        let rich_address = default_signer()?.address();
        wait_for_balance(&provider, rich_address, Duration::from_secs(60)).await?;

        Ok(ExternalNode {
            repo_root: self.repo_root.clone(),
            server_binary: self.server_binary.clone(),
            rpc_url,
            status_url,
            server_log_path: server_log,
            artifacts_dir,
            server,
            node_config,
        })
    }

    pub fn l1_rpc_url(&self) -> &str {
        &self.l1_rpc_url
    }

    pub fn main_node_record(&self) -> &str {
        &self.main_node_record
    }
}

impl Drop for TestEnvironment {
    fn drop(&mut self) {
        if let Some(server) = self.server.as_mut() {
            server.kill_nonblocking();
        }
        self.anvil.kill_nonblocking();
    }
}

impl ChildProcess {
    async fn kill(&mut self) {
        let _ = self.child.kill().await;
        let _ = self.child.wait().await;
    }

    fn kill_nonblocking(&mut self) {
        let _ = self.child.start_kill();
    }
}

impl Drop for ChildProcess {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
    }
}

impl ExternalNode {
    pub fn provider(&self) -> anyhow::Result<impl Provider> {
        default_provider(self.rpc_url.clone())
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
            .context("failed to submit transaction to external node")?;
        let tx_hash = pending.tx_hash().to_string();
        let receipt = pending
            .get_receipt()
            .await
            .context("external-node transaction was not included in time")?;
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

    pub async fn get_transaction_receipt_json(
        &self,
        tx_hash: B256,
    ) -> anyhow::Result<Option<serde_json::Value>> {
        let client = reqwest::Client::new();
        let response = client
            .post(&self.rpc_url)
            .json(&json!({
                "jsonrpc": "2.0",
                "method": "eth_getTransactionReceipt",
                "params": [format!("{tx_hash:#x}")],
                "id": 1
            }))
            .send()
            .await
            .context("failed to call eth_getTransactionReceipt on external node")?;
        let response: serde_json::Value = response.json().await?;
        Ok(response["result"].clone().as_object().map(|_| response["result"].clone()))
    }

    pub async fn wait_for_receipt_json(&self, tx_hash: B256) -> anyhow::Result<serde_json::Value> {
        let started = Instant::now();
        loop {
            if let Some(receipt) = self.get_transaction_receipt_json(tx_hash).await? {
                return Ok(receipt);
            }
            if started.elapsed() >= Duration::from_secs(120) {
                anyhow::bail!("timed out waiting for external-node receipt for {tx_hash:#x}");
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    pub async fn send_basic_transfer(&self) -> anyhow::Result<B256> {
        let provider = self.provider()?;
        let pending = provider
            .send_transaction(
                TransactionRequest::default()
                    .with_to(Address::repeat_byte(0x11))
                    .with_value(U256::from(1_u64)),
            )
            .await
            .context("failed to submit transaction to external node")?;
        Ok(*pending.tx_hash())
    }

    pub async fn restart(&mut self, l1_rpc_url: &str, main_node_rpc_url: &str, main_node_record: &str) -> anyhow::Result<()> {
        if let Some(server) = self.server.as_mut() {
            server.kill().await;
        }
        let server = spawn_external_node(
            &self.repo_root,
            &self.server_binary,
            l1_rpc_url,
            main_node_rpc_url,
            main_node_record,
            &self.node_config,
            &self.server_log_path,
        )
        .await
        .context("failed to restart external node")?;
        self.server = Some(server);
        wait_for_health(&self.status_url, Duration::from_secs(120)).await?;
        Ok(())
    }
}

impl Drop for ExternalNode {
    fn drop(&mut self) {
        if let Some(server) = self.server.as_mut() {
            server.kill_nonblocking();
        }
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

async fn spawn_main_node(
    repo_root: &Path,
    server_binary: &Path,
    l1_rpc_url: &str,
    config: &NodeProcessConfig,
    log_path: &Path,
) -> anyhow::Result<ChildProcess> {
    let config_path = repo_root.join("local-chains/v30.2/default/config.yaml");
    let mut server_cmd = Command::new(server_binary);
    server_cmd
        .arg("--config")
        .arg(config_path)
        .current_dir(repo_root)
        .env("general_l1_rpc_url", l1_rpc_url)
        .env("general_rocks_db_path", &config.rocks_db_path)
        .env("rpc_address", format!("127.0.0.1:{}", config.rpc_port))
        .env("network_enabled", "true")
        .env("network_address", "127.0.0.1")
        .env("network_port", config.network_port.to_string())
        .env("network_secret_key", secret_key_to_hex(&config.network_secret_key))
        .env("status_server_enabled", "true")
        .env("status_server_address", format!("127.0.0.1:{}", config.status_port))
        .env("observability_prometheus_port", config.prometheus_port.to_string())
        .env("prover_api_address", format!("127.0.0.1:{}", config.prover_api_port))
        .env("prover_api_proof_storage_path", &config.proof_storage_path)
        .env("RUST_LOG", "info");
    if let Some(block_time) = config.block_time {
        server_cmd.env("sequencer_block_time", format_duration(block_time));
    }
    let server = spawn_logged("server", &mut server_cmd, log_path).await?;
    wait_for_health(&main_node_status_url(config), Duration::from_secs(120)).await?;
    Ok(server)
}

async fn spawn_external_node(
    repo_root: &Path,
    server_binary: &Path,
    l1_rpc_url: &str,
    main_node_rpc_url: &str,
    main_node_record: &str,
    config: &NodeProcessConfig,
    log_path: &Path,
) -> anyhow::Result<ChildProcess> {
    let config_path = repo_root.join("local-chains/v30.2/default/config.yaml");
    let mut server_cmd = Command::new(server_binary);
    server_cmd
        .arg("--config")
        .arg(config_path)
        .current_dir(repo_root)
        .env("general_l1_rpc_url", l1_rpc_url)
        .env("general_rocks_db_path", &config.rocks_db_path)
        .env("general_node_role", "external")
        .env("general_main_node_rpc_url", main_node_rpc_url)
        .env("rpc_address", format!("127.0.0.1:{}", config.rpc_port))
        .env("network_enabled", "true")
        .env("network_address", "127.0.0.1")
        .env("network_port", config.network_port.to_string())
        .env("network_secret_key", secret_key_to_hex(&config.network_secret_key))
        .env(
            "network_boot_nodes__JSON",
            serde_json::to_string(&vec![main_node_record.to_owned()])?,
        )
        .env("l1_sender_pubdata_mode__JSON", "null")
        .env("status_server_enabled", "true")
        .env("status_server_address", format!("127.0.0.1:{}", config.status_port))
        .env("observability_prometheus_port", config.prometheus_port.to_string())
        .env("prover_api_address", format!("127.0.0.1:{}", config.prover_api_port))
        .env("prover_api_proof_storage_path", &config.proof_storage_path)
        .env("RUST_LOG", "info");
    let server = spawn_logged("external-node", &mut server_cmd, log_path).await?;
    wait_for_health(&main_node_status_url(config), Duration::from_secs(120)).await?;
    Ok(server)
}

async fn ensure_server_binary(repo_root: &Path) -> anyhow::Result<PathBuf> {
    let metadata = Command::new("cargo")
        .arg("metadata")
        .arg("--manifest-path")
        .arg(repo_root.join("Cargo.toml"))
        .arg("--format-version")
        .arg("1")
        .arg("--no-deps")
        .current_dir(repo_root)
        .output()
        .await
        .context("failed to query cargo metadata for zksync-os-server")?;
    if !metadata.status.success() {
        anyhow::bail!(
            "cargo metadata failed with status {}",
            metadata.status
        );
    }

    let metadata_json: serde_json::Value = serde_json::from_slice(&metadata.stdout)
        .context("failed to parse cargo metadata output")?;
    let target_directory = metadata_json["target_directory"]
        .as_str()
        .ok_or_else(|| anyhow!("cargo metadata missing target_directory"))?;
    let binary_path = PathBuf::from(target_directory).join("debug/zksync-os-server");
    if binary_path.exists() {
        return Ok(binary_path);
    }

    let status = Command::new("cargo")
        .arg("build")
        .arg("--manifest-path")
        .arg(repo_root.join("Cargo.toml"))
        .arg("--bin")
        .arg("zksync-os-server")
        .current_dir(repo_root)
        .status()
        .await
        .context("failed to build zksync-os-server")?;
    if !status.success() {
        anyhow::bail!("building zksync-os-server failed with status {status}");
    }

    if !binary_path.exists() {
        anyhow::bail!("expected server binary at {}", binary_path.display());
    }

    Ok(binary_path)
}

fn default_signer() -> anyhow::Result<PrivateKeySigner> {
    RICH_PRIVATE_KEY.parse().context("failed to parse rich private key")
}

fn default_provider(rpc_url: String) -> anyhow::Result<impl Provider + Clone> {
    provider_for_rpc_and_signer(rpc_url, default_signer()?)
}

pub fn provider_for_rpc_and_signer(
    rpc_url: String,
    signer: PrivateKeySigner,
) -> anyhow::Result<impl Provider + Clone> {
    let wallet = EthereumWallet::from(signer);
    let url = Url::parse(&rpc_url)?;
    let provider = ProviderBuilder::new().wallet(wallet).connect_http(url);
    provider.client().set_poll_interval(Duration::from_millis(10));
    Ok(provider)
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

fn main_node_rpc_url(config: &NodeProcessConfig) -> String {
    format!("http://127.0.0.1:{}", config.rpc_port)
}

fn main_node_status_url(config: &NodeProcessConfig) -> String {
    format!("http://127.0.0.1:{}/status/health", config.status_port)
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

fn secret_key_to_hex(secret_key: &B256) -> String {
    format!("{secret_key:#x}")
}

fn random_network_secret_key() -> B256 {
    let signing_key = SigningKey::random(&mut OsRng);
    let bytes = signing_key.to_bytes();
    B256::from_slice(&bytes)
}

fn enode_from_secret_key(secret_key: &B256, port: u16) -> String {
    let signing_key =
        SigningKey::from_slice(secret_key.as_slice()).expect("generated secret key must be valid");
    let verifying_key = signing_key.verifying_key();
    let encoded = verifying_key.to_encoded_point(false);
    let public_key = encoded
        .as_bytes()
        .strip_prefix(&[0x04])
        .expect("uncompressed key has 0x04 prefix");
    format!("enode://{}@127.0.0.1:{port}", hex::encode(public_key))
}
