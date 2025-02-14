#!/bin/bash


CSV_FILE="metrics.csv"
if [ ! -f "$CSV_FILE" ]; then
    echo "block,count,total_txs,total_cycles,user_cycles,paging_cycles,witness_size,blobs,pre_images,cycles_per_tx" > "$CSV_FILE"
fi

awk '
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
}' >> $CSV_FILE
