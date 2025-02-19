#!/bin/bash
set -euo pipefail

# Configuration
ERC20_CONTRACT=$(cat token_address.txt)
TRANSFER_AMOUNT="100000"
FROM_ADDRESS="0x9965507D1a55bcC2695C58ba16FB37d819B0A4dc"
PRIVATE_KEY="0x8b3a350cf5c34c9194ca85829a2df0ec3153be0318b5e2d3348e872092edffba"
L2_RPC="http://127.0.0.1:9545"

# Validate args
[[ $# -eq 1 ]] || {
    echo "Usage: $0 <concurrent_threads>"
    exit 1
}
concurrent_threads=$1 # note that this input is essentially how many tx/block. hardware dependent.
num_txs=$((concurrent_threads * 100))

# Signal handling
cleanup() {
    rm -rf "$tmpdir"
    kill $(jobs -p) 2>/dev/null || true
    exit 1
}
trap cleanup SIGINT SIGTERM

tmpdir=$(mktemp -d)
current_nonce=$(cast nonce --rpc-url "$L2_RPC" "$FROM_ADDRESS")

# Prepare transactions
echo "Creating $num_txs transactions to deploy $concurrent_threads txs per block..."
tx_count=0
while IFS= read -r recipient && ((tx_count < num_txs)); do
    [[ -z "$recipient" || "$recipient" =~ ^# ]] && continue

    echo "Preparing tx $tx_count to recipient: $recipient"

    signed_tx=$(cast mktx \
        --private-key "$PRIVATE_KEY" \
        --rpc-url "$L2_RPC" \
        --nonce "$current_nonce" \
        --gas-limit 100000 \
        "$ERC20_CONTRACT" \
        "transfer(address,uint256)" \
        "$recipient" \
        "$TRANSFER_AMOUNT")

    echo "$signed_tx" >"$tmpdir/$tx_count"
    ((current_nonce++))
    ((tx_count++))

    ((tx_count % 100 == 0)) && echo "Prepared $tx_count transactions"
done <addresses.txt

echo "Submitting $tx_count transactions with $concurrent_threads threads..."

# Submit transactions
for ((i = 0; i < tx_count; i = i + concurrent_threads)); do
    batch_end=$((i + concurrent_threads))
    ((batch_end > tx_count)) && batch_end=$tx_count

    for ((j = i; j < batch_end; j++)); do
        (cast publish --rpc-url "$L2_RPC" "$(cat "$tmpdir/$j")" &&
            echo "Published tx $j") &
    done
    wait
done

cleanup
echo "Transfer complete - processed $tx_count transactions"
