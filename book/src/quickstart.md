# Quickstart

Kailua enables rollup operators to add a new fault proof system to their rollup using the `DisputeGameFactory`.
Kailua's contracts rely on RISC-Zero zkVM proofs to finalize/dismiss output proposals, and are compatible with Optimism's Bedrock contracts `v1.4.0` and above.

The `KailuaTournament` contract optimistically allows outputs to be accepted after a timeout if no fraud proof is published against it,
or if the output is challenged, waits for a proof to be submitted to decide whether to dismiss the output.

## Prerequisites
1. [rust](https://www.rust-lang.org/tools/install)
2. [just](https://just.systems/man/en/)
3. [docker](https://www.docker.com/)
4. [solc](https://docs.soliditylang.org/en/latest/installing-solidity.html)
5. [foundry](https://book.getfoundry.sh/getting-started/installation)

## Devnet Usage

1. `just devnet-install`
    * Fetches `v1.9.1` of the `optimism` monorepo
2. `just devnet-build`
    * Builds the local cargo and foundry projects
3. `just devnet-up`
    * Starts a local OP Stack devnet using docker.
    * Dumps the output into `devnetlog.txt` for inspection.
4. `just devnet-upgrade`
    * Upgrades the devnet to use the `KailuaGame` contract.
    * Assumes the default values of the local optimism devnet, but can take parameters.
5. `just devnet-propose`
    * Launches the kailua proposer.
    * This constantly creates new `KailuaGame` instances to finalize the l2 on the l1.
6. `just devnet-validate`
    * Launches the kailua validator.
    * This challenges `KailuaGame` instances that contain invalid proposals
7. `just devnet-fault`
    * Deploys a single `KailuaGame` instance with a faulty proposal.
    * Tests the validator's fault proving functionality.
    * Tests the proposer's canonical chain decision functionality.
8. After you're done:
    * `just devnet-down` to stop the running docker containers
    * `just devnet-clean` to cleanup the docker volumes