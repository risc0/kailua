#!/bin/bash
# Configuration
L2_RPC="http://127.0.0.1:9545"
ROLLUP_NODE_RPC="http://127.0.0.1:7545"

function print_block_info() {
    local block_info=$1

    # Use awk to extract all needed fields in one pass
    local block_data=$(echo "$block_info" | awk '
        /number/      {number=$2}
        /hash/        {hash=$2}
        /gasUsed/     {gasUsed=$2}
        /gasLimit/    {gasLimit=$2}
        /baseFeePerGas/ {baseFee=$2}
        /transactions:/ {
            tx_count=0
            while (getline && $0 != "]") {
                if ($0 ~ /0x/) {
                    txs[tx_count] = $0
                    tx_count++
                }
            }
        }
        END {
            printf "Block #%s:\n", number
            printf "  Hash: %s\n", hash
            printf "  Gas Used: %s / %s (%.2f%%)\n", gasUsed, gasLimit, (gasUsed * 100 / gasLimit)
            printf "  Base Fee: %s\n", baseFee
            printf "  Transaction Count: %d\n", tx_count
            if (tx_count > 0) {
                print "  Transactions:"
                for (i = 0; i < tx_count; i++) {
                    printf "    %s\n", txs[i]
                }
            }
        }
    ')

    echo "$block_data"
    echo "----------------------------------------"
}

function get_block_info() {
    local block_num=$1
    cast block --rpc-url $L2_RPC $block_num
}

function watch_blocks() {
    local last_processed_block=""

    while true; do
        # Get latest block number
        local latest_block_info=$(cast block --rpc-url $L2_RPC latest)
        local latest_block_num=$(echo "$latest_block_info" | awk '/number/{print $2}')

        # If this is our first run, initialize last_processed_block
        if [ -z "$last_processed_block" ]; then
            last_processed_block=$((latest_block_num - 1))
        fi

        # Process all blocks between last_processed_block and latest_block_num
        while [ $last_processed_block -lt $latest_block_num ]; do
            last_processed_block=$((last_processed_block + 1))
            local block_info=$(get_block_info $last_processed_block)

            if [ ! -z "$block_info" ]; then
                print_block_info "$block_info"
            else
                echo "Failed to get info for block $last_processed_block"
            fi
        done

        sleep 0.1
    done
}

function watch_finalization() {
    local last_finalized=0

    while true; do
        local sync_status
        sync_status=$(cast rpc --rpc-url $ROLLUP_NODE_RPC "optimism_syncStatus" 2>/dev/null)

        if [ ! -z "$sync_status" ]; then
            local finalized_l2_number
            local finalized_l1_number
            finalized_l2_number=$(echo "$sync_status" | jq -r '.finalized_l2.number')
            finalized_l1_number=$(echo "$sync_status" | jq -r '.finalized_l1.number')

            # Check for new finalizations
            if [ "$finalized_l2_number" -gt "$last_finalized" ]; then
                # Print all newly finalized blocks
                for block_num in $(seq $((last_finalized + 1)) $finalized_l2_number); do
                    echo "L2 Block #$block_num finalized on L1 Block #$finalized_l1_number"
                done
                last_finalized=$finalized_l2_number
            fi
        fi

        sleep 1
    done
}

case "$1" in
"watch-blocks")
    watch_blocks
    ;;
"watch-finalization")
    watch_finalization
    ;;
*)
    echo "Usage: $0 {watch-blocks | watch-finalization}"
    exit 1
    ;;
esac
