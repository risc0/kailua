#!/bin/bash

L2_RPC="http://127.0.0.1:9545"
ROLLUP_NODE_RPC="http://127.0.0.1:7545"
PRIVATE_KEY="0x8b3a350cf5c34c9194ca85829a2df0ec3153be0318b5e2d3348e872092edffba"
FROM_ADDRESS="0x9965507D1a55bcC2695C58ba16FB37d819B0A4dc"
TO_ADDRESS="0x70997970C51812dc3A010C7d01b50e0d17dc79C8"
TOKEN_ADDRESS=$(cat token_address.txt)
TX_PER_BLOCK=1
LAST_REPORTED_BLOCK=0
TRACKED_L2_BLOCKS=()

trap 'jobs -p | xargs -r kill; exit 0' SIGINT SIGTERM EXIT

echo "Deploying token..."
forge script script/TestToken.s.sol:DeployToken --rpc-url $L2_RPC --private-key $PRIVATE_KEY --broadcast

echo "Minting tokens..."
forge script script/TestToken.s.sol:MintTokens --rpc-url $L2_RPC --private-key $PRIVATE_KEY --broadcast

check_finalization() {
    local l2_block=$1
    local sync_status
    sync_status=$(cast rpc --rpc-url $ROLLUP_NODE_RPC "optimism_syncStatus" 2>/dev/null)
    local finalized_l2_number
    finalized_l2_number=$(echo "$sync_status" | jq -r '.finalized_l2.number')
    
    if [ "$l2_block" -le "$finalized_l2_number" ]; then
        local finalized_l1_number
        finalized_l1_number=$(echo "$sync_status" | jq -r '.finalized_l1.number')
        echo "L2 Block #$l2_block finalized on L1 Block #$finalized_l1_number"
        TRACKED_L2_BLOCKS=(${TRACKED_L2_BLOCKS[@]/$l2_block})
    fi
}

send_batch_transactions() {
    local start_nonce=$1
    local batch_size=$2
    local pids=()
    
    for ((i=0; i<batch_size; i++)); do
        cast send --private-key $PRIVATE_KEY \
            --rpc-url $L2_RPC \
            --nonce $((start_nonce + i)) \
            --gas-limit 100000 \
            $TOKEN_ADDRESS \
            "transfer(address,uint256)" \
            $TO_ADDRESS \
            1000000000000000000 &>/dev/null &
            
        pids+=($!)
    done
    
    wait "${pids[@]}" 2>/dev/null
}

tx_sender() {
    local current_nonce=$(cast nonce --rpc-url $L2_RPC $FROM_ADDRESS)
    local last_send=0
    
    while true; do
        current_time=$(date +%s)
        if ((current_time - last_send >= 1)); then
            send_batch_transactions $current_nonce $TX_PER_BLOCK
            current_nonce=$((current_nonce + TX_PER_BLOCK))
            last_send=$current_time
        fi
    done
}

tx_sender &

while true; do
    l2_block=$(cast block-number --rpc-url $L2_RPC)
    
    if [ "$l2_block" != "$LAST_REPORTED_BLOCK" ]; then
        block_data=$(cast block $l2_block --rpc-url $L2_RPC --json)
        tx_count=$(echo "$block_data" | jq '.transactions | length')
        gas_used=$(echo "$block_data" | jq -r '.gasUsed')
        
        if [ "$tx_count" -gt 0 ]; then
            TRACKED_L2_BLOCKS+=($l2_block)
            echo "Block #$l2_block - Txs: $tx_count - Gas: $gas_used"
        fi
        LAST_REPORTED_BLOCK=$l2_block
    fi
    
    for block in "${TRACKED_L2_BLOCKS[@]}"; do
        check_finalization "$block"
    done
    
    # sleep 1
done
