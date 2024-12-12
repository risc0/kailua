set fallback := true

# default recipe to display help information
default:
  @just --list

build +ARGS="--release":
  cargo build {{ARGS}}

clippy:
  RISC0_SKIP_BUILD=1 cargo clippy --workspace --all --all-features --all-targets -- -D warnings

kurtosis-up:
  kurtosis run github.com/ethpandaops/optimism-package --args-file kurtosis.yaml > kurtosis.log

kurtosis-down:
  kurtosis clean -a

devnet-install:
  git clone --depth 1 --branch v1.9.1 --recursive https://github.com/ethereum-optimism/optimism.git

devnet-build +ARGS="--release -F devnet": (build ARGS)

devnet-up:
  make -C optimism devnet-up > devnet.log

devnet-down:
  make -C optimism devnet-down

devnet-clean: devnet-down
  make -C optimism devnet-clean

devnet-upgrade verbosity="" l1_rpc="http://127.0.0.1:8545" l1_beacon_rpc="http://127.0.0.1:5052" l2_rpc="http://127.0.0.1:9545" rollup_node_rpc="http://127.0.0.1:7545" registry="0xd801426328C609fCDe6E3B7a5623C27e8F607832" portal="0x6509f2a854BA7441039fCE3b959d5bAdd2fFCFCD" deployer="0x8b3a350cf5c34c9194ca85829a2df0ec3153be0318b5e2d3348e872092edffba" owner="0x7c852118294e51e653712a81e05800f419141751be58f605c371e15141b007a6" guardian="0x2a871d0798f97d79848a013d4936a73bf4cc922c825d33c1cf7073dff6d409c6":
  ./target/debug/kailua-cli deploy \
      --registry-contract {{registry}} \
      --portal-contract {{portal}} \
      --l1-node-address {{l1_rpc}} \
      --l1-beacon-address {{l1_beacon_rpc}} \
      --l2-node-address {{l2_rpc}} \
      --op-node-address {{rollup_node_rpc}} \
      --deployer-key {{deployer}} \
      --owner-key {{owner}} \
      --guardian-key {{guardian}} \
      {{verbosity}}

devnet-reset:
  just devnet-down

  just devnet-clean

  just devnet-up

devnet-propose verbosity="-vv" l1_rpc="http://127.0.0.1:8545" l1_beacon_rpc="http://127.0.0.1:5052" rollup_node_rpc="http://127.0.0.1:7545" data_dir=".localtestdata/propose" registry="0xd801426328C609fCDe6E3B7a5623C27e8F607832" deployer="0x8b3a350cf5c34c9194ca85829a2df0ec3153be0318b5e2d3348e872092edffba":
  ./target/debug/kailua-cli propose \
      --registry-contract {{registry}} \
      --l1-node-address {{l1_rpc}} \
      --l1-beacon-address {{l1_beacon_rpc}} \
      --op-node-address {{rollup_node_rpc}} \
      --data-dir {{data_dir}} \
      --proposer-key {{deployer}} \
      {{verbosity}}

devnet-fault verbosity="-vv" offset="1" parent="1" l1_rpc="http://127.0.0.1:8545" l1_beacon_rpc="http://127.0.0.1:5052" rollup_node_rpc="http://127.0.0.1:7545" registry="0xd801426328C609fCDe6E3B7a5623C27e8F607832" deployer="0x47e179ec197488593b187f80a00eb0da91f1b9d0b13f8733639f19c30a34926a":
  ./target/debug/kailua-cli test-fault \
      --registry-contract {{registry}} \
      --l1-node-address {{l1_rpc}} \
      --l1-beacon-address {{l1_beacon_rpc}} \
      --op-node-address {{rollup_node_rpc}} \
      --proposer-key {{deployer}} \
      --fault-offset {{offset}} \
      --fault-parent {{parent}} \
      {{verbosity}}

devnet-validate verbosity="-vv" l1_rpc="http://127.0.0.1:8545" l1_beacon_rpc="http://127.0.0.1:5052" l2_rpc="http://127.0.0.1:9545" rollup_node_rpc="http://127.0.0.1:7545" data_dir=".localtestdata/validate" kailua_host="./target/debug/kailua-host" registry="0xd801426328C609fCDe6E3B7a5623C27e8F607832" validator="0x8b3a350cf5c34c9194ca85829a2df0ec3153be0318b5e2d3348e872092edffba":
  ./target/debug/kailua-cli validate \
      --registry-contract {{registry}} \
      --l1-node-address {{l1_rpc}} \
      --l1-beacon-address {{l1_beacon_rpc}} \
      --l2-node-address {{l2_rpc}} \
      --op-node-address {{rollup_node_rpc}} \
      --kailua-host {{kailua_host}} \
      --validator-key {{validator}} \
      {{verbosity}}

devnet-prove block_number verbosity="-vv" data=".localtestdata":
  just prove {{block_number}} http://localhost:8545 http://localhost:5052 http://localhost:9545 http://localhost:7545 {{data}} {{verbosity}}

bench l1_rpc l1_beacon_rpc l2_rpc rollup_node_rpc data start range count verbosity="-v":
    ./target/debug/kailua-cli benchmark \
          --l1-node-address {{l1_rpc}} \
          --l1-beacon-address {{l1_beacon_rpc}} \
          --l2-node-address {{l2_rpc}} \
          --op-node-address {{rollup_node_rpc}} \
          --data-dir {{data}} \
          --bench-start {{start}} \
          --bench-range {{range}} \
          --bench-count {{count}} \
          {{verbosity}}

# Run the client program natively with the host program attached.
prove block_number block_count l1_rpc l1_beacon_rpc l2_rpc rollup_node_rpc data verbosity="":
  #!/usr/bin/env bash

  L1_NODE_ADDRESS="{{l1_rpc}}"
  L1_BEACON_ADDRESS="{{l1_beacon_rpc}}"
  L2_NODE_ADDRESS="{{l2_rpc}}"
  OP_NODE_ADDRESS="{{rollup_node_rpc}}"

  L2_BLOCK_NUMBER={{block_number}}
  CLAIMED_L2_BLOCK_NUMBER=$((L2_BLOCK_NUMBER + {{block_count}} - 1))

  # Query the chain id
  echo "Fetching chain id"
  L2_CHAIN_ID=$(cast chain-id --rpc-url $L2_NODE_ADDRESS)

  # Get output root for block
  echo "Fetching data for block #$CLAIMED_L2_BLOCK_NUMBER..."
  CLAIMED_L2_OUTPUT_ROOT=$(cast rpc --rpc-url $OP_NODE_ADDRESS "optimism_outputAtBlock" $(cast 2h $CLAIMED_L2_BLOCK_NUMBER) | jq -r .outputRoot)
  # Get the info for the origin l1 block
  L1_ORIGIN_NUM=$(cast rpc --rpc-url $OP_NODE_ADDRESS "optimism_outputAtBlock" $(cast 2h $CLAIMED_L2_BLOCK_NUMBER) | jq -r .blockRef.l1origin.number)
  L1_HEAD=$(cast block --rpc-url $L1_NODE_ADDRESS $((L1_ORIGIN_NUM + 50)) -j | jq -r .hash)

  # Get the info for the parent l2 block
  echo "Fetching data for parent of block #$L2_BLOCK_NUMBER..."
  AGREED_L2_OUTPUT_ROOT=$(cast rpc --rpc-url $OP_NODE_ADDRESS "optimism_outputAtBlock" $(cast 2h $((L2_BLOCK_NUMBER - 1))) | jq -r .outputRoot)
  AGREED_L2_HEAD=$(cast block --rpc-url $L2_NODE_ADDRESS $((L2_BLOCK_NUMBER - 1)) -j | jq -r .hash)

  echo "Running host program with zk client program..."
  ./target/debug/kailua-host \
    --l1-head $L1_HEAD \
    --agreed-l2-head-hash $AGREED_L2_HEAD \
    --agreed-l2-output-root $AGREED_L2_OUTPUT_ROOT \
    --claimed-l2-output-root $CLAIMED_L2_OUTPUT_ROOT \
    --claimed-l2-block-number $CLAIMED_L2_BLOCK_NUMBER \
    --block-count {{block_count}} \
    --l2-chain-id $L2_CHAIN_ID \
    --l1-node-address $L1_NODE_ADDRESS \
    --l1-beacon-address $L1_BEACON_ADDRESS \
    --l2-node-address $L2_NODE_ADDRESS \
    --op-node-address $OP_NODE_ADDRESS \
    --data-dir {{data}} \
    --native \
    {{verbosity}}

prove-devnet block_number data verbosity="":
  #!/usr/bin/env bash

  just prove {{block_number}} http://localhost:8545 http://localhost:5052 http://localhost:9545 http://localhost:7545 {{data}} {{verbosity}}

prove-kurtosis block_number data verbosity="":
  #!/usr/bin/env bash

  just prove {{block_number}} http://127.0.0.1:63638 http://127.0.0.1:63650 http://127.0.0.1:49320 http://127.0.0.1:49383 {{data}} {{verbosity}}

# Show the input args for proving
query block_number l1_rpc l1_beacon_rpc l2_rpc rollup_node_rpc:
  #!/usr/bin/env bash

  L1_NODE_ADDRESS="{{l1_rpc}}"
  L1_BEACON_ADDRESS="{{l1_beacon_rpc}}"
  L2_NODE_ADDRESS="{{l2_rpc}}"
  OP_NODE_ADDRESS="{{rollup_node_rpc}}"

  L2_BLOCK_NUMBER={{block_number}}

  echo "Fetching data for block #$L2_BLOCK_NUMBER..."
  L1_ORIGIN_NUM=$(cast rpc --rpc-url $OP_NODE_ADDRESS "optimism_outputAtBlock" $(cast 2h $((L2_BLOCK_NUMBER - 1))) | jq -r .blockRef.l1origin.number)

  # L2 Claim
  cast rpc --rpc-url $OP_NODE_ADDRESS "optimism_outputAtBlock" $(cast 2h $L2_BLOCK_NUMBER) | jq -r .outputRoot
  # L2 output root
  cast rpc --rpc-url $OP_NODE_ADDRESS "optimism_outputAtBlock" $(cast 2h $((L2_BLOCK_NUMBER - 1))) | jq -r .outputRoot
  # L2 head
  cast block --rpc-url $L2_NODE_ADDRESS $((L2_BLOCK_NUMBER - 1)) -j | jq -r .hash
  # L1 head
  cast block --rpc-url $L1_NODE_ADDRESS $((L1_ORIGIN_NUM + 50)) -j | jq -r .hash
  # L2 chain id
  cast chain-id --rpc-url $L2_NODE_ADDRESS

prove-offline block_number l2_claim l2_output_root l2_head l1_head l2_chain_id data verbosity="":
  echo "Running host program with zk client program..."
  ./target/debug/kailua-host \
    --l1-head {{l1_head}} \
    --agreed-l2-head-hash {{l2_head}} \
    --claimed-l2-output-root {{l2_claim}} \
    --agreed-l2-output-root {{l2_output_root}} \
    --claimed-l2-block-number {{block_number}} \
    --l2-chain-id {{l2_chain_id}} \
    --data-dir {{data}} \
    --native \
    {{verbosity}}

test verbosity="":
    echo "Rebuilding kailua using cargo"
    just devnet-build

    echo "Running cargo tests"
    RISC0_DEV_MODE=1 cargo test -F devnet

test-offline verbosity="":
    echo "Running offline proof for op-sepolia block 16491249 in dev-mode"
    just prove-offline 16491249 0x82da7204148ba4d8d59e587b6b3fdde5561dc31d9e726220f7974bf9f2158d75 0xa548f22e1aa590de7ed271e3eab5b66c6c3db9b8cb0e3f91618516ea9ececde4 0x09b298a83baf4c2e3c6a2e355bb09e27e3fdca435080e8754f8749233d7333b2 0x33a3e5721faa4dc6f25e75000d9810fd6c41320868f3befcc0c261a71da398e1 11155420 ./testdata/16491249 {{verbosity}}

cleanup:
    echo "Cleanup: Removing any .fake receipt files in directory."
    rm ./*.fake
