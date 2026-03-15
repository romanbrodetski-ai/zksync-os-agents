use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Ecosystem-level contracts.yaml structure (subset of fields we need).
#[derive(Debug, Deserialize)]
pub struct EcosystemContracts {
    pub core_ecosystem_contracts: CoreEcosystemContracts,
    pub zksync_os_ctm: Option<ZksyncOsCtm>,
    pub bridges: Option<Bridges>,
}

#[derive(Debug, Deserialize)]
pub struct CoreEcosystemContracts {
    pub bridgehub_proxy_addr: String,
    pub native_token_vault_addr: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ZksyncOsCtm {
    pub state_transition_proxy_addr: Option<String>,
    pub validator_timelock_addr: Option<String>,
    pub l1_bytecodes_supplier_addr: Option<String>,
    pub blobs_zksync_os_l1_da_validator_addr: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct Bridges {
    pub shared: Option<BridgeAddresses>,
}

#[derive(Debug, Deserialize)]
pub struct BridgeAddresses {
    pub l1_address: Option<String>,
}

/// Chain-level wallets.yaml structure.
#[derive(Debug, Deserialize)]
pub struct ChainWallets {
    pub deployer: Option<Wallet>,
    pub operator: Option<Wallet>,
    pub blob_operator: Option<Wallet>,
    pub prove_operator: Option<Wallet>,
    pub execute_operator: Option<Wallet>,
    pub fee_account: Option<Wallet>,
    pub governor: Option<Wallet>,
}

#[derive(Debug, Deserialize)]
pub struct Wallet {
    pub address: String,
    pub private_key: String,
}

/// Chain-level contracts.yaml (subset).
#[derive(Debug, Deserialize)]
pub struct ChainContracts {
    pub l1: Option<ChainL1Contracts>,
}

#[derive(Debug, Deserialize)]
pub struct ChainL1Contracts {
    pub diamond_proxy_addr: Option<String>,
}

/// Generated server config structure.
#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct ServerConfig {
    pub general: GeneralConfig,
    pub genesis: GenesisConfig,
    pub l1_sender: L1SenderConfig,
    pub sequencer: SequencerConfig,
    pub external_price_api_client: ExternalPriceApiClientConfig,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct GeneralConfig {
    pub l1_rpc_url: String,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct GenesisConfig {
    pub bridgehub_address: String,
    pub bytecode_supplier_address: Option<String>,
    pub genesis_input_path: String,
    pub chain_id: u64,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct L1SenderConfig {
    pub operator_commit_sk: Option<String>,
    pub operator_prove_sk: Option<String>,
    pub operator_execute_sk: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct SequencerConfig {
    pub fee_collector_address: String,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct ExternalPriceApiClientConfig {
    pub source: String,
    pub forced_prices: HashMap<String, u64>,
}

/// Generate a server config from ecosystem and chain config files.
pub fn generate_server_config(
    eco_contracts: &EcosystemContracts,
    chain_wallets: &ChainWallets,
    chain_id: u64,
    genesis_input_path: &str,
    l1_rpc_url: &str,
) -> ServerConfig {
    let bytecodes_supplier = eco_contracts
        .zksync_os_ctm
        .as_ref()
        .and_then(|ctm| ctm.l1_bytecodes_supplier_addr.clone());

    // Key mapping: blob_operator→commit, prove_operator→prove, execute_operator→execute
    let commit_sk = chain_wallets
        .blob_operator
        .as_ref()
        .map(|w| w.private_key.clone());
    let prove_sk = chain_wallets
        .prove_operator
        .as_ref()
        .map(|w| w.private_key.clone());
    let execute_sk = chain_wallets
        .execute_operator
        .as_ref()
        .map(|w| w.private_key.clone());

    let fee_address = chain_wallets
        .fee_account
        .as_ref()
        .map(|w| w.address.clone())
        .unwrap_or_else(|| "0x0000000000000000000000000000000000000000".to_string());

    let mut forced_prices = HashMap::new();
    forced_prices.insert(
        "0x0000000000000000000000000000000000000001".to_string(),
        3000,
    );

    ServerConfig {
        general: GeneralConfig {
            l1_rpc_url: l1_rpc_url.to_string(),
        },
        genesis: GenesisConfig {
            bridgehub_address: eco_contracts
                .core_ecosystem_contracts
                .bridgehub_proxy_addr
                .clone(),
            bytecode_supplier_address: bytecodes_supplier,
            genesis_input_path: genesis_input_path.to_string(),
            chain_id,
        },
        l1_sender: L1SenderConfig {
            operator_commit_sk: commit_sk,
            operator_prove_sk: prove_sk,
            operator_execute_sk: execute_sk,
        },
        sequencer: SequencerConfig {
            fee_collector_address: fee_address,
        },
        external_price_api_client: ExternalPriceApiClientConfig {
            source: "Forced".to_string(),
            forced_prices,
        },
    }
}
