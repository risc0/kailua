#!/bin/bash
set -euo pipefail

# Configuration
ERC20_CONTRACT=$(cat erc20_address.txt)
TRANSFER_AMOUNT="100"
FROM_ADDRESS="0x9965507D1a55bcC2695C58ba16FB37d819B0A4dc"
PRIVATE_KEY="0x8b3a350cf5c34c9194ca85829a2df0ec3153be0318b5e2d3348e872092edffba"
L2_RPC="http://127.0.0.1:9545"
TOTAL_BLOCKS=124

[[ $# -eq 1 ]] || {
    echo "Usage: $0 <txs_per_block>"
    exit 1
}
TXS_PER_BLOCK=$1
TOTAL_TXS=$((TOTAL_BLOCKS * TXS_PER_BLOCK))

cleanup() {
    rm -rf "$tmpdir"
    kill $(jobs -p) 2>/dev/null || true
    exit 1
}
trap cleanup SIGINT SIGTERM

tmpdir=$(mktemp -d)
current_nonce=$(cast nonce --rpc-url "$L2_RPC" "$FROM_ADDRESS")

get_block_number() {
    curl -s -X POST -H "Content-Type: application/json" \
        --data '{"jsonrpc":"2.0","method":"eth_blockNumber","params":[],"id":1}' \
        "$L2_RPC" | jq -r '.result'
}

submit_tx() {
    local raw_tx=$1
    curl -s -X POST -H "Content-Type: application/json" \
        --data "{\"jsonrpc\":\"2.0\",\"method\":\"eth_sendRawTransaction\",\"params\":[\"$raw_tx\"],\"id\":1}" \
        "$L2_RPC"
}

echo "Preparing $TOTAL_TXS transactions for $TOTAL_BLOCKS blocks..."
tx_count=0
while IFS= read -r recipient && ((tx_count < TOTAL_TXS)); do
    [[ -z "$recipient" || "$recipient" =~ ^# ]] && continue
    
    signed_tx=$(cast mktx \
        --private-key "$PRIVATE_KEY" \
        --rpc-url "$L2_RPC" \
        --nonce "$current_nonce" \
        --gas-limit 100000 \
        "$ERC20_CONTRACT" \
        "transfer(address,uint256)" \
        "$recipient" \
        "$TRANSFER_AMOUNT")
    
    echo "Preparing tx $tx_count to recipient: $recipient"
    
    echo "$signed_tx" > "$tmpdir/$tx_count"
    ((current_nonce++))
    ((tx_count++))
done < addresses.txt

echo "Starting block-controlled submission ($TXS_PER_BLOCK tx/block)..."
current_tx=0
blocks_processed=0

while ((blocks_processed < TOTAL_BLOCKS)); do
    initial_block=$(get_block_number)
    
    for ((i = 0; i < TXS_PER_BLOCK; i++)); do
        submit_tx "$(cat "$tmpdir/$current_tx")"
        ((current_tx++))
    done
    
    while [[ "$(get_block_number)" == "$initial_block" ]]; do
        sleep 0.1
    done
    
    ((blocks_processed++))
    echo "Submitted $TXS_PER_BLOCK transactions in block $initial_block" | tee -a publisher.log
done

cleanup
echo "Transfer complete - processed $tx_count transactions across $TOTAL_BLOCKS blocks"