use zksync_os_sepolia_deploy_tests::config_gen::*;

fn sample_ecosystem_contracts() -> EcosystemContracts {
    serde_yaml::from_str(
        r#"
core_ecosystem_contracts:
  bridgehub_proxy_addr: "0x1d9490c2f6513843bc6d694c0b499be5e2779b87"
  native_token_vault_addr: "0x76316df18b4b2ed4b69e981827d756e008088de5"
zksync_os_ctm:
  state_transition_proxy_addr: "0x5672953c9736dad421753cf25aec20df0ff77c3d"
  validator_timelock_addr: "0x9ef840870068923270959e6a35705a5e08a64500"
  l1_bytecodes_supplier_addr: "0x024c7e6c9c2eff05be9e3586118de257e29b920c"
  blobs_zksync_os_l1_da_validator_addr: "0x70cb09e928995c9a63d38e5fe2209e4f0803900f"
bridges:
  shared:
    l1_address: "0xc4aa880607166faf6ee3d1fba6a1e62cf3fe88bf"
"#,
    )
    .unwrap()
}

fn sample_chain_wallets() -> ChainWallets {
    serde_yaml::from_str(
        r#"
deployer:
  address: "0xe3b62ba528bc1c57320486c283bee67d3b59b8be"
  private_key: "0x82e31f9618ee70d88188ea76df05400886021a5e0fda7f79872301984a02ad4a"
operator:
  address: "0xb088e46d1a63c80ab93d636600230ce05b6f9915"
  private_key: "0x74a73bf6b120782853ce6e5e14439c5820e9a0ba323083e1f37473d69b64be64"
blob_operator:
  address: "0xe8e2655a5cfdf204a3b86b8fc1ddf64aa3c02923"
  private_key: "0x811829e543221db5ed75d9b62ee7067cf68e4ffba0d0cd0fd2df90e7ef17cb68"
prove_operator:
  address: "0xfa6556d71b0364fa28aeb22779c9c81980bf2883"
  private_key: "0x87646f93c29f76dcc4a23f79adcc0dd20c35263e994d89aec726f47d505855c6"
execute_operator:
  address: "0x6adad5a8cf0055c70eb4a85219a3726e318443dd"
  private_key: "0x2182a05b96a734a7407ddf555adef73c6a278f3b1aa0ebd8f87a3634234b66f4"
fee_account:
  address: "0x2d2ba8f62ebf8a006733e93492eea8fd650a6e4a"
  private_key: "0xab38e39d715b4b23d22af3f613135e97ded7dc816839298231c1c69b0ccedea8"
governor:
  address: "0x2f2b4b59328e5a0affbcaac0bab39c7baa6abbc8"
  private_key: "0xaf2114e20a2beb607bfce1849089829de6a4322c3205f1f8af4d66896755c62a"
"#,
    )
    .unwrap()
}

/// Test 1.1: Bridgehub address extracted from ecosystem contracts
#[test]
fn bridgehub_address_extracted_correctly() {
    let eco = sample_ecosystem_contracts();
    let wallets = sample_chain_wallets();
    let config = generate_server_config(&eco, &wallets, 837101, "./genesis.json", "http://rpc");

    assert_eq!(
        config.genesis.bridgehub_address,
        "0x1d9490c2f6513843bc6d694c0b499be5e2779b87"
    );
    // Must NOT use shared bridge address or any other address
    assert_ne!(
        config.genesis.bridgehub_address,
        "0xc4aa880607166faf6ee3d1fba6a1e62cf3fe88bf",
        "Should not use shared bridge address as bridgehub"
    );
}

/// Test 1.2: Bytecodes supplier address extracted
#[test]
fn bytecodes_supplier_extracted() {
    let eco = sample_ecosystem_contracts();
    let wallets = sample_chain_wallets();
    let config = generate_server_config(&eco, &wallets, 837101, "./genesis.json", "http://rpc");

    assert_eq!(
        config.genesis.bytecode_supplier_address.as_deref(),
        Some("0x024c7e6c9c2eff05be9e3586118de257e29b920c")
    );
}

/// Test 1.3: Operator key role mapping is correct
/// blob_operator → commit, prove_operator → prove, execute_operator → execute
#[test]
fn operator_key_role_mapping() {
    let eco = sample_ecosystem_contracts();
    let wallets = sample_chain_wallets();
    let config = generate_server_config(&eco, &wallets, 837101, "./genesis.json", "http://rpc");

    // blob_operator key must be commit_sk
    assert_eq!(
        config.l1_sender.operator_commit_sk.as_deref(),
        Some("0x811829e543221db5ed75d9b62ee7067cf68e4ffba0d0cd0fd2df90e7ef17cb68"),
        "commit_sk must be blob_operator's key (has committer role)"
    );

    // prove_operator key must be prove_sk
    assert_eq!(
        config.l1_sender.operator_prove_sk.as_deref(),
        Some("0x87646f93c29f76dcc4a23f79adcc0dd20c35263e994d89aec726f47d505855c6"),
        "prove_sk must be prove_operator's key (has prover role)"
    );

    // execute_operator key must be execute_sk
    assert_eq!(
        config.l1_sender.operator_execute_sk.as_deref(),
        Some("0x2182a05b96a734a7407ddf555adef73c6a278f3b1aa0ebd8f87a3634234b66f4"),
        "execute_sk must be execute_operator's key (has executor role)"
    );

    // Critically: commit_sk must NOT be the regular operator key
    assert_ne!(
        config.l1_sender.operator_commit_sk.as_deref(),
        Some("0x74a73bf6b120782853ce6e5e14439c5820e9a0ba323083e1f37473d69b64be64"),
        "commit_sk must not be the generic operator key (would cause L1 revert)"
    );
}

/// Test 1.4: Chain ID propagated correctly
#[test]
fn chain_id_propagated() {
    let eco = sample_ecosystem_contracts();
    let wallets = sample_chain_wallets();
    let config = generate_server_config(&eco, &wallets, 837101, "./genesis.json", "http://rpc");

    assert_eq!(config.genesis.chain_id, 837101);
    // Must not use the default local chain ID
    assert_ne!(config.genesis.chain_id, 6565, "Must not use default local chain ID");
}

/// Test 1.5: Genesis path is set
#[test]
fn genesis_path_set() {
    let eco = sample_ecosystem_contracts();
    let wallets = sample_chain_wallets();
    let path = "./sepolia_ecosystem/chains/zkos_sepolia/configs/genesis.json";
    let config = generate_server_config(&eco, &wallets, 837101, path, "http://rpc");

    assert_eq!(config.genesis.genesis_input_path, path);
}

/// Test: Fee collector address from fee_account wallet
#[test]
fn fee_collector_from_fee_account() {
    let eco = sample_ecosystem_contracts();
    let wallets = sample_chain_wallets();
    let config = generate_server_config(&eco, &wallets, 837101, "./genesis.json", "http://rpc");

    assert_eq!(
        config.sequencer.fee_collector_address,
        "0x2d2ba8f62ebf8a006733e93492eea8fd650a6e4a"
    );
}

/// Test: L1 RPC URL propagated
#[test]
fn l1_rpc_url_propagated() {
    let eco = sample_ecosystem_contracts();
    let wallets = sample_chain_wallets();
    let rpc = "https://l1-api-sepolia-1.zksync-nodes.com";
    let config = generate_server_config(&eco, &wallets, 837101, "./genesis.json", rpc);

    assert_eq!(config.general.l1_rpc_url, rpc);
}

/// Test: Config round-trips through YAML serialization
#[test]
fn config_yaml_round_trip() {
    let eco = sample_ecosystem_contracts();
    let wallets = sample_chain_wallets();
    let original = generate_server_config(
        &eco,
        &wallets,
        837101,
        "./genesis.json",
        "https://rpc.example.com",
    );

    let yaml_str = serde_yaml::to_string(&original).unwrap();
    let deserialized: ServerConfig = serde_yaml::from_str(&yaml_str).unwrap();

    assert_eq!(original, deserialized);
}

/// Test: Missing optional wallets produce None keys (not panic)
#[test]
fn missing_optional_wallets_handled() {
    let eco = sample_ecosystem_contracts();
    let wallets: ChainWallets = serde_yaml::from_str(
        r#"
operator:
  address: "0xb088e46d1a63c80ab93d636600230ce05b6f9915"
  private_key: "0x74a73bf6b120782853ce6e5e14439c5820e9a0ba323083e1f37473d69b64be64"
"#,
    )
    .unwrap();

    let config = generate_server_config(&eco, &wallets, 837101, "./genesis.json", "http://rpc");

    assert!(config.l1_sender.operator_commit_sk.is_none());
    assert!(config.l1_sender.operator_prove_sk.is_none());
    assert!(config.l1_sender.operator_execute_sk.is_none());
}

/// Test: Missing zksync_os_ctm → no bytecodes supplier
#[test]
fn missing_ctm_no_bytecodes_supplier() {
    let eco: EcosystemContracts = serde_yaml::from_str(
        r#"
core_ecosystem_contracts:
  bridgehub_proxy_addr: "0x1d9490c2f6513843bc6d694c0b499be5e2779b87"
"#,
    )
    .unwrap();
    let wallets = sample_chain_wallets();
    let config = generate_server_config(&eco, &wallets, 837101, "./genesis.json", "http://rpc");

    assert!(config.genesis.bytecode_supplier_address.is_none());
}
