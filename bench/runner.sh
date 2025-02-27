#!/bin/bash

if [ "$#" -ne 1 ]; then
    echo "Usage: $0 <starting_block>"
    exit 1
fi

BLOCK=$1                 # input arg block start
COUNTS=(1024 1280 1536 1792 2048) # block counts
TIMESTAMP=$(date +%Y%m%d)
CSV_FILE="metrics_${TIMESTAMP}.csv"

echo "Starting measurements at $(date)"
echo "Output will be saved to: $CSV_FILE"

if [ ! -f "$CSV_FILE" ]; then
    echo "Creating new metrics file..."
    echo "total_cycles,user_cycles,witness_size,n_blocks,total_txs,total_gas,n_blobs" >"$CSV_FILE"
fi

for count in "${COUNTS[@]}"; do
    echo "Processing block $BLOCK with count $count..."
    echo "----------------------------------------"

    RISC0_RV32IM_VER=1 RISC0_DEV_MODE=1 RUST_LOG=info RISC0_INFO=1 just devnet-prove $BLOCK $count | tee >(awk '/total cycles/ {total_cycles=$7} /user cycles/ {split($10, a, "("); user_cycles=a[2]} /Witness size:/ {witness_size=$7} /Processing job with/ {n_blocks=$8} /Proving .* transactions for/ {total_txs=$6; total_gas=$9} /BEACON/ {blobs=$3} END {print total_cycles "," user_cycles "," witness_size "," n_blocks "," total_txs "," total_gas "," blobs; }'>>"$CSV_FILE")

    if [ $? -ne 0 ]; then
        echo "Failed at count $count"
        exit 1
    fi

    echo "Completed count $count"
    echo "----------------------------------------"
done

echo "Metrics saved to $CSV_FILE"
