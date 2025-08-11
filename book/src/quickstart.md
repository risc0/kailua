# Quickstart

Kailua enables rollup operators to add a new fault proof system to their rollup via the Optimism `DisputeGameFactory`
contract.
Kailua's contracts rely on RISC-Zero zkVM proofs to finalize/dismiss output proposals, and are compatible with
Optimism's Bedrock contracts `v1.4.0` and above.


## Prerequisites
1. [rust](https://www.rust-lang.org/tools/install)
2. [just](https://just.systems/man/en/)
3. [docker](https://www.docker.com/)
4. [svm](https://github.com/alloy-rs/svm-rs)
5. [foundry](https://book.getfoundry.sh/getting-started/installation)

## Live Chain

You can test out Kailua's validity proving on a running chain through the following commands:

1. `just build`
   * Compiles a release build of Kailua
2. `just demo [BLOCKS_PER_PROOF] [L1_RPC] [BEACON_RPC] [L2_RPC] [OP_NODE_RPC]:`
   * Runs the release build against the target chain endpoints.
   * See [here](validator.md#delegated-proof-generation) for advanced proving configuration

## Local Devnet

You can deploy a local optimism devnet equipped with Kailua through the following commands:

1. `just devnet-fetch`
    * Fetches `v1.9.1` of the `optimism` monorepo.
2. `just devnet-build`
    * Builds the local cargo and foundry projects.
3. `just devnet-up`
    * Starts a local OP Stack devnet using Docker.
    * Dumps the output into `devnetlog.txt` for inspection.
4. `just devnet-upgrade`
    * Upgrades the devnet to use the `KailuaGame` contract.
    * Assumes the default values of the local optimism devnet, but can take parameters.
5. `just devnet-propose`
    * Launches the Kailua proposer.
    * This runs the sequences, which periodically creates new `KailuaGame` instances.
6. `just devnet-validate`
    * Launches the Kailua validator.
    * This monitors `KailuaGame` instances for disputes and creates proofs to resolve them.
    * (VALIDITY PROVING) Use `just devnet-validate [block-height]` to generate validity proofs to fast-forward finality until the specified L2 block height.
    * (DEVELOPMENT MODE): Use `RISC0_DEV_MODE=1` to use fake proofs.
7. `just devnet-rpc`
    * Launches the Kailua RPC.
    * This provides utility RPC methods for initiating withdrawals.
    * Listens on http://127.0.0.1:1337 and ws://127.0.0.1:1337 by default.
8. `just devnet-fault`
    * Deploys a single `KailuaGame` instance with a faulty sequencing proposal.
    * Tests the validator's fault proving functionality.
    * Tests the proposer's canonical chain tracking functionality.
9. After you're done:
    * `just devnet-down` to stop the running docker containers.
    * `just devnet-clean` to cleanup the docker volumes.
