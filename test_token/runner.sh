#!/bin/bash

if [ "$#" -ne 1 ]; then
    echo "Usage: $0 <starting_block>"
    exit 1
fi

BLOCK=$1                 # input arg block start
COUNTS=(3 6 12 24 48 64) # block counts
TIMESTAMP=$(date +%Y%m%d)
CSV_FILE="metrics_${TIMESTAMP}.csv"

echo "Starting measurements at $(date)"
echo "Output will be saved to: $CSV_FILE"

if [ ! -f "$CSV_FILE" ]; then
    echo "Creating new metrics file..."
    echo "block,count,total_txs,total_cycles,user_cycles,paging_cycles,witness_size,blobs,pre_images,cycles_per_tx" >"$CSV_FILE"
fi

for count in "${COUNTS[@]}"; do
    echo "Processing block $BLOCK with count $count..."
    echo "----------------------------------------"

    RISC0_RV32IM_VER=2 RISC0_DEV_MODE=1 RUST_LOG=info RISC0_INFO=1 just devnet-prove $BLOCK $count | tee >(awk '
    /Witness size:/ {
        witness_size=$NF
    } 
    /BEACON:.*BLOBS/ {
        split($0, a, "BEACON: "); 
        split(a[2], b, " "); 
        blobs=b[1];
    } 
    /Proof of/ { 
        total_cycles = $7; 
        split($10, a, "(");
        user_cycles = a[2];
    }
    /ORACLE:.*PREIMAGES/ {
        split($0, a, "ORACLE: "); 
        split(a[2], b, " "); 
        pre_imgs = b[1];
    }
    /Fetching data for parent of block/ {
        split($0, a, "#");
        split(a[2], b, /\./);
        block = b[1];
    }
    /Processing job with/ {
        split($0, a, "with "); 
        split(a[2], b, " blocks"); 
        count = b[1];
    }
    /Proving .*transactions over/ {
        split($0, a, "Proving ");                   
        split(a[2], b, " transactions"); 
        txs = b[1];
    }
    /paging cycles/ {
        paging_cycles = $5;
    }
    END {
        cycles_per_tx = total_cycles / txs;  
        print block "," count "," txs "," total_cycles "," user_cycles "," paging_cycles "," witness_size "," blobs "," pre_imgs "," cycles_per_tx;
    }' >>"$CSV_FILE")

    if [ $? -ne 0 ]; then
        echo "Failed at count $count"
        exit 1
    fi

    echo "Completed count $count"
    echo "----------------------------------------"
done

echo "Metrics saved to $CSV_FILE"
