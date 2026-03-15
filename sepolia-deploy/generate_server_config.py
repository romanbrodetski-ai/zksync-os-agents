#!/usr/bin/env python3
"""
Generate a zksync-os-server config.yaml from zkstack ecosystem init output.

Usage:
    python3 generate_server_config.py <ecosystem_dir> [--l1-rpc-url URL] [--output PATH]

Example:
    python3 generate_server_config.py ./sepolia_ecosystem \
        --l1-rpc-url https://l1-api-sepolia-1.zksync-nodes.com \
        --output config.yaml
"""
import argparse
import sys
import yaml
from pathlib import Path


def load_yaml(path: Path) -> dict:
    with open(path) as f:
        return yaml.safe_load(f)


def to_hex(value, width: int = 40) -> str:
    """Convert a value to 0x-prefixed hex string, zero-padded to *width* hex chars.

    PyYAML parses unquoted 0x... values as integers.  This converts them back.
    Addresses are 40 hex chars (20 bytes), private keys are 64 hex chars (32 bytes).
    """
    if isinstance(value, int):
        return "0x" + format(value, f"0{width}x")
    return str(value)


def find_chain_dir(ecosystem_dir: Path) -> Path:
    """Find the single chain directory under chains/."""
    chains_dir = ecosystem_dir / "chains"
    if not chains_dir.exists():
        sys.exit(f"Error: {chains_dir} does not exist")
    chain_dirs = [d for d in chains_dir.iterdir() if d.is_dir()]
    if len(chain_dirs) == 0:
        sys.exit(f"Error: no chain directories found in {chains_dir}")
    if len(chain_dirs) > 1:
        print(f"Warning: multiple chains found, using first: {chain_dirs[0].name}", file=sys.stderr)
    return chain_dirs[0]


def generate_config(ecosystem_dir: Path, l1_rpc_url: str) -> dict:
    """Generate server config from zkstack ecosystem output."""
    ecosystem_dir = Path(ecosystem_dir)
    chain_dir = find_chain_dir(ecosystem_dir)

    # Load zkstack output files
    eco_contracts = load_yaml(ecosystem_dir / "configs" / "contracts.yaml")
    chain_contracts = load_yaml(chain_dir / "configs" / "contracts.yaml")
    chain_wallets = load_yaml(chain_dir / "configs" / "wallets.yaml")

    # Discover genesis.json path
    genesis_path = chain_dir / "configs" / "genesis.json"
    if not genesis_path.exists():
        # Fall back to ecosystem-level genesis
        for p in ecosystem_dir.glob("**/genesis.json"):
            genesis_path = p
            break

    # Extract bridgehub address
    bridgehub_raw = eco_contracts.get("core_ecosystem_contracts", {}).get("bridgehub_proxy_addr")
    if not bridgehub_raw:
        sys.exit("Error: bridgehub_proxy_addr not found in ecosystem contracts.yaml")
    bridgehub = to_hex(bridgehub_raw)

    # Extract bytecodes supplier
    bytecodes_supplier_raw = eco_contracts.get("zksync_os_ctm", {}).get("l1_bytecodes_supplier_addr")
    if not bytecodes_supplier_raw:
        # Try alternative location
        bytecodes_supplier_raw = eco_contracts.get("l1_bytecodes_supplier_addr")
    if not bytecodes_supplier_raw:
        print("Warning: l1_bytecodes_supplier_addr not found, server may fail to fetch genesis upgrade tx", file=sys.stderr)
        bytecodes_supplier = None
    else:
        bytecodes_supplier = to_hex(bytecodes_supplier_raw)

    # Extract chain ID: try chain-level ZkStack.yaml first, then genesis.json
    chain_zkstack = chain_dir / "ZkStack.yaml"
    chain_id = None
    if chain_zkstack.exists():
        chain_meta = load_yaml(chain_zkstack)
        chain_id = chain_meta.get("chain_id")

    if chain_id is None:
        # Try from genesis.json
        import json
        with open(genesis_path) as f:
            genesis_data = json.load(f)
            chain_id = genesis_data.get("l2_chain_id")
    if chain_id is None:
        sys.exit("Error: could not determine chain_id")

    # Map operator keys by role
    # blob_operator → commit (has committer role on ValidatorTimelock)
    # prove_operator → prove
    # execute_operator → execute
    blob_op = chain_wallets.get("blob_operator", {})
    prove_op = chain_wallets.get("prove_operator", {})
    execute_op = chain_wallets.get("execute_operator", {})
    fee_account = chain_wallets.get("fee_account", {})

    commit_sk = to_hex(blob_op["private_key"], 64) if blob_op.get("private_key") else None
    prove_sk = to_hex(prove_op["private_key"], 64) if prove_op.get("private_key") else None
    execute_sk = to_hex(execute_op["private_key"], 64) if execute_op.get("private_key") else None

    if not all([commit_sk, prove_sk, execute_sk]):
        print("Warning: some operator private keys missing from wallets.yaml", file=sys.stderr)

    fee_raw = fee_account.get("address")
    fee_address = to_hex(fee_raw) if fee_raw else "0x0000000000000000000000000000000000000000"

    # Build config
    config = {
        "general": {
            "l1_rpc_url": l1_rpc_url,
        },
        "genesis": {
            "bridgehub_address": bridgehub,
            "chain_id": chain_id,
            "genesis_input_path": str(genesis_path),
        },
        "l1_sender": {},
        "sequencer": {
            "fee_collector_address": fee_address,
        },
        "external_price_api_client": {
            "source": "Forced",
            "forced_prices": {
                "0x0000000000000000000000000000000000000001": 3000,
            },
        },
    }

    if bytecodes_supplier:
        config["genesis"]["bytecode_supplier_address"] = bytecodes_supplier

    if commit_sk:
        config["l1_sender"]["operator_commit_sk"] = commit_sk
    if prove_sk:
        config["l1_sender"]["operator_prove_sk"] = prove_sk
    if execute_sk:
        config["l1_sender"]["operator_execute_sk"] = execute_sk

    return config


def main():
    parser = argparse.ArgumentParser(description="Generate zksync-os-server config from zkstack ecosystem output")
    parser.add_argument("ecosystem_dir", help="Path to the ecosystem directory (contains ZkStack.yaml)")
    parser.add_argument("--l1-rpc-url", default="https://l1-api-sepolia-1.zksync-nodes.com",
                        help="L1 RPC URL (default: Sepolia)")
    parser.add_argument("--output", "-o", default="-", help="Output file (default: stdout)")
    args = parser.parse_args()

    config = generate_config(args.ecosystem_dir, args.l1_rpc_url)

    # Use a representer that quotes strings starting with 0x to prevent
    # them from being parsed as integers on re-load.
    class HexDumper(yaml.SafeDumper):
        pass

    def _represent_str(dumper, data):
        if data.startswith("0x"):
            return dumper.represent_scalar("tag:yaml.org,2002:str", data, style="'")
        return dumper.represent_scalar("tag:yaml.org,2002:str", data)

    HexDumper.add_representer(str, _represent_str)

    output = yaml.dump(config, Dumper=HexDumper, default_flow_style=False, sort_keys=False)

    if args.output == "-":
        print(output)
    else:
        with open(args.output, "w") as f:
            f.write(output)
        print(f"Config written to {args.output}", file=sys.stderr)


if __name__ == "__main__":
    main()
