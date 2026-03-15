#!/usr/bin/env bash
# deploy.sh — End-to-end Sepolia chain deployment and verification
#
# Usage:
#   ./deploy.sh <ecosystem_dir> [--l1-rpc-url URL] [--zkstack PATH] [--server-binary PATH] [--skip-deploy] [--skip-fund] [--timeout SECS]
#
# Prerequisites:
#   - era-contracts compiled (forge build)
#   - zkstack CLI built
#   - zksync-os-server built
#   - Funded deployer/governor wallets in ecosystem config
#
# Environment:
#   DEPLOYER_SK — private key for funding operators (defaults to deployer from ecosystem wallets)
#   L1_RPC_URL — L1 RPC endpoint (default: https://l1-api-sepolia-1.zksync-nodes.com)

set -euo pipefail

# ─── Defaults ──────────────────────────────────────────────────────────
L1_RPC_URL="${L1_RPC_URL:-https://l1-api-sepolia-1.zksync-nodes.com}"
ZKSTACK="${ZKSTACK:-zkstack}"
SERVER_BINARY="${SERVER_BINARY:-target/release/zksync-os-server}"
SKIP_DEPLOY=false
SKIP_FUND=false
SETTLEMENT_TIMEOUT=180  # seconds to wait for first batch execute

# ─── Parse args ────────────────────────────────────────────────────────
ECOSYSTEM_DIR=""
while [[ $# -gt 0 ]]; do
    case "$1" in
        --l1-rpc-url)   L1_RPC_URL="$2"; shift 2;;
        --zkstack)      ZKSTACK="$2"; shift 2;;
        --server-binary) SERVER_BINARY="$2"; shift 2;;
        --skip-deploy)  SKIP_DEPLOY=true; shift;;
        --skip-fund)    SKIP_FUND=true; shift;;
        --timeout)      SETTLEMENT_TIMEOUT="$2"; shift 2;;
        -*)             echo "Unknown flag: $1" >&2; exit 1;;
        *)              ECOSYSTEM_DIR="$1"; shift;;
    esac
done

if [[ -z "$ECOSYSTEM_DIR" ]]; then
    echo "Usage: $0 <ecosystem_dir> [flags]" >&2
    exit 1
fi

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
LOG_DIR="$(mktemp -d)"
echo "Logs: $LOG_DIR"

# ─── Step 1: Deploy ecosystem ─────────────────────────────────────────
if [[ "$SKIP_DEPLOY" == false ]]; then
    echo "=== Step 1: Deploying ecosystem contracts ==="
    # Remove stale contracts.yaml to force fresh deploy
    rm -f "$ECOSYSTEM_DIR/configs/contracts.yaml"

    cd "$ECOSYSTEM_DIR"
    $ZKSTACK ecosystem init \
        --deploy-paymaster=false --deploy-erc20=false --observability=false \
        --no-port-reallocation --deploy-ecosystem \
        --l1-rpc-url="$L1_RPC_URL" \
        --zksync-os --ignore-prerequisites --skip-contract-compilation-override \
        --no-genesis --validium-type=no-da \
        2>&1 | tee "$LOG_DIR/ecosystem-init.log"
    cd - >/dev/null

    echo "=== Ecosystem deployed ==="
else
    echo "=== Step 1: Skipped (--skip-deploy) ==="
fi

# ─── Step 2: Generate server config ───────────────────────────────────
echo "=== Step 2: Generating server config ==="
CONFIG_FILE="$ECOSYSTEM_DIR/server-config.yaml"
python3 "$SCRIPT_DIR/generate_server_config.py" "$ECOSYSTEM_DIR" \
    --l1-rpc-url "$L1_RPC_URL" \
    --output "$CONFIG_FILE"
echo "Config written to $CONFIG_FILE"

# ─── Step 3: Fund operator wallets ────────────────────────────────────
if [[ "$SKIP_FUND" == false ]]; then
    echo "=== Step 3: Funding operator wallets ==="

    # Find chain wallets
    CHAIN_DIR=$(find "$ECOSYSTEM_DIR/chains" -maxdepth 1 -mindepth 1 -type d | head -1)
    WALLETS_FILE="$CHAIN_DIR/configs/wallets.yaml"

    # Extract deployer key (use DEPLOYER_SK env or from ecosystem wallets)
    if [[ -z "${DEPLOYER_SK:-}" ]]; then
        ECO_WALLETS="$ECOSYSTEM_DIR/configs/wallets.yaml"
        DEPLOYER_SK=$(python3 -c "
import yaml
with open('$ECO_WALLETS') as f:
    w = yaml.safe_load(f)
print(w.get('deployer', {}).get('private_key', ''))
")
    fi

    if [[ -z "$DEPLOYER_SK" ]]; then
        echo "Warning: No deployer key found, skipping funding" >&2
    else
        # Extract operator addresses
        BLOB_OP=$(python3 -c "import yaml; w=yaml.safe_load(open('$WALLETS_FILE')); print(w['blob_operator']['address'])")
        PROVE_OP=$(python3 -c "import yaml; w=yaml.safe_load(open('$WALLETS_FILE')); print(w['prove_operator']['address'])")
        EXEC_OP=$(python3 -c "import yaml; w=yaml.safe_load(open('$WALLETS_FILE')); print(w['execute_operator']['address'])")

        FUND_AMOUNT="0.2ether"
        for ADDR in "$BLOB_OP" "$PROVE_OP" "$EXEC_OP"; do
            BAL=$(cast balance "$ADDR" --rpc-url "$L1_RPC_URL" --ether 2>/dev/null || echo "0")
            if (( $(echo "$BAL < 0.1" | bc -l 2>/dev/null || echo 1) )); then
                echo "  Funding $ADDR with $FUND_AMOUNT..."
                cast send "$ADDR" --value "$FUND_AMOUNT" --private-key "$DEPLOYER_SK" --rpc-url "$L1_RPC_URL" --legacy >/dev/null 2>&1
            else
                echo "  $ADDR already funded ($BAL ETH)"
            fi
        done
        echo "=== Operators funded ==="
    fi
else
    echo "=== Step 3: Skipped (--skip-fund) ==="
fi

# ─── Step 4: Clean DB and start server ────────────────────────────────
echo "=== Step 4: Starting server ==="
rm -rf ./db/node1
$SERVER_BINARY --config "$CONFIG_FILE" > "$LOG_DIR/server.log" 2>&1 &
SERVER_PID=$!
echo "Server PID: $SERVER_PID"

cleanup() {
    echo "Stopping server (PID $SERVER_PID)..."
    kill "$SERVER_PID" 2>/dev/null || true
    wait "$SERVER_PID" 2>/dev/null || true
}
trap cleanup EXIT

# Wait for RPC to come up
echo "  Waiting for JSON-RPC..."
for i in $(seq 1 30); do
    if curl -s -X POST -H "Content-Type: application/json" \
        -d '{"jsonrpc":"2.0","method":"eth_chainId","params":[],"id":1}' \
        http://localhost:3050 >/dev/null 2>&1; then
        echo "  JSON-RPC is up"
        break
    fi
    sleep 1
done

# ─── Step 5: Wait for L1 settlement ──────────────────────────────────
echo "=== Step 5: Waiting for L1 batch settlement (timeout: ${SETTLEMENT_TIMEOUT}s) ==="
START_TIME=$(date +%s)
COMMITTED=false
PROVED=false
EXECUTED=false

while true; do
    NOW=$(date +%s)
    ELAPSED=$((NOW - START_TIME))
    if [[ $ELAPSED -ge $SETTLEMENT_TIMEOUT ]]; then
        echo "TIMEOUT after ${SETTLEMENT_TIMEOUT}s"
        break
    fi

    if [[ "$COMMITTED" == false ]] && grep -q "succeeded on L1.*command=commit batch 1" "$LOG_DIR/server.log" 2>/dev/null; then
        COMMITTED=true
        COMMIT_LINE=$(grep "succeeded on L1.*command=commit batch 1" "$LOG_DIR/server.log" | head -1)
        COMMIT_GAS=$(echo "$COMMIT_LINE" | grep -oP 'gas_used=\K[0-9]+')
        COMMIT_FEE=$(echo "$COMMIT_LINE" | grep -oP 'l1_transaction_fee_ether="\K[^"]+')
        echo "  [${ELAPSED}s] Batch 1 COMMITTED (gas: $COMMIT_GAS, fee: $COMMIT_FEE ETH)"
    fi

    if [[ "$PROVED" == false ]] && grep -q "succeeded on L1.*command=prove batches 1" "$LOG_DIR/server.log" 2>/dev/null; then
        PROVED=true
        PROVE_LINE=$(grep "succeeded on L1.*command=prove batches 1" "$LOG_DIR/server.log" | head -1)
        PROVE_GAS=$(echo "$PROVE_LINE" | grep -oP 'gas_used=\K[0-9]+')
        PROVE_FEE=$(echo "$PROVE_LINE" | grep -oP 'l1_transaction_fee_ether="\K[^"]+')
        echo "  [${ELAPSED}s] Batch 1 PROVED   (gas: $PROVE_GAS, fee: $PROVE_FEE ETH)"
    fi

    if [[ "$EXECUTED" == false ]] && grep -q "succeeded on L1.*command=execute batches 1" "$LOG_DIR/server.log" 2>/dev/null; then
        EXECUTED=true
        EXEC_LINE=$(grep "succeeded on L1.*command=execute batches 1" "$LOG_DIR/server.log" | head -1)
        EXEC_GAS=$(echo "$EXEC_LINE" | grep -oP 'gas_used=\K[0-9]+')
        EXEC_FEE=$(echo "$EXEC_LINE" | grep -oP 'l1_transaction_fee_ether="\K[^"]+')
        echo "  [${ELAPSED}s] Batch 1 EXECUTED (gas: $EXEC_GAS, fee: $EXEC_FEE ETH)"
        break
    fi

    # Check if server died
    if ! kill -0 "$SERVER_PID" 2>/dev/null; then
        echo "ERROR: Server process died" >&2
        tail -20 "$LOG_DIR/server.log" >&2
        exit 1
    fi

    sleep 2
done

# ─── Step 6: Report ──────────────────────────────────────────────────
echo ""
echo "=== Deployment Report ==="
echo ""

if [[ "$EXECUTED" == true ]]; then
    echo "Status: SUCCESS - Full L1 settlement verified"
    echo ""
    echo "Settlement Gas Costs (Batch 1):"
    echo "  Commit:  ${COMMIT_GAS:-?} gas (${COMMIT_FEE:-?} ETH)"
    echo "  Prove:   ${PROVE_GAS:-?} gas (${PROVE_FEE:-?} ETH)"
    echo "  Execute: ${EXEC_GAS:-?} gas (${EXEC_FEE:-?} ETH)"
    TOTAL_GAS=$(( ${COMMIT_GAS:-0} + ${PROVE_GAS:-0} + ${EXEC_GAS:-0} ))
    echo "  Total:   $TOTAL_GAS gas"
    echo ""

    # Check for batch 2
    if grep -q "succeeded on L1.*command=execute batches 2" "$LOG_DIR/server.log" 2>/dev/null; then
        echo "Batch 2 also settled (steady-state verified)"
    fi
    EXIT_CODE=0
elif [[ "$COMMITTED" == true ]]; then
    echo "Status: PARTIAL - Committed but not fully settled"
    EXIT_CODE=1
else
    echo "Status: FAILED - No batches committed"
    echo "Last 20 lines of server log:"
    tail -20 "$LOG_DIR/server.log"
    EXIT_CODE=1
fi

echo ""
echo "Logs: $LOG_DIR"
echo "Config: $CONFIG_FILE"
echo "Server log: $LOG_DIR/server.log"

exit $EXIT_CODE
