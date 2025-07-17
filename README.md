<img width="100%" src="https://github.com/user-attachments/assets/c71a1018-d970-49c0-af37-ba8a66d00bea" />

# Kailua
Unlock faster finality and stronger security for your OP chain.

> [!NOTE]
> Documentation: https://risc0.github.io/kailua/


## Intro

OP Kailua lets OP chains upgrade to ZK with minimal friction, unlocking faster finality and enhanced security. With two configurable modes, rollups can adopt ZK at their own pace while meeting the proving requirements for stage 2 decentralization.

* Hybrid Mode → Uses ZK Fraud Proofs for faster finality and lower operational costs.
* Validity Mode → Uses ZK Validity Proofs for 1-Hour finality and maximum security.

## Why OP Kailua?
✅ **Pick Your Security Model**: Start with Hybrid Mode, upgrade to Validity Mode when ready. \
✅ **Fast & Easy Integration**: Works with Optimism’s Bedrock contracts (v1.4.0+).\
✅ **Stage 2 Ready**: Future-proof your rollup and accelerate your track to Stage 2 Decentralization

## How does OP Kailua Work?
**Hybrid Mode**: ZK Fraud Proofs for Faster Dispute Resolution

OP Kailua Hybrid Mode replaces traditional fault-proof mechanisms with ZK Fraud proofs, improving security without requiring rollups to immediately switch to full validity proofs.

- Uses the RISC-Zero zkVM to verifiably run Optimism's [Kona][kona] and secure rollups with cryptographic proofs enabling faster finality and reduced operational costs.
- Designed to require constant collateral lockups from both proposers and validators (challengers), whereas the Bisection-based fault dispute game backed by Cannon requires a linear number of deposits proportional to the number of proposals/challenges.
- The fault proofs are estimated to require on the order of 100 billion cycles to prove in the worst case, which, on Bonsai, would cost on the order of 100 USD and take around an hour to prove.
- All proving costs are borne by the dishonest party in the protocol, whether that is the proposer or validator.


**Validity Mode**: ZK Validity Proofs for 1-Hour Finality

Validity Mode turns OP chains into a ZK Rollup, eliminating disputes entirely since every transaction is validity proven.

- Unlocks 1-hour finality
- No challenges, no disputes—ZK Validity Proofs replace interactive fault proofs.


## Audits

Kailua has undergone the following audits throughout its development:
* [18 FEB 2025](https://github.com/risc0/rz-security/blob/main/audits/kailua/veridise-kailua-20250217.pdf)
* [22 MAY 2025](https://github.com/risc0/rz-security/blob/main/audits/kailua/veridise-kailua-20250522.pdf)
* [16 JUN 2025](https://github.com/risc0/rz-security/blob/main/audits/kailua/veridise-kailua-20250616.pdf)

## Prerequisites
1. [rust](https://www.rust-lang.org/tools/install)
2. [just](https://just.systems/man/en/)
3. [docker](https://www.docker.com/)
4. [svm](https://github.com/alloy-rs/svm-rs)
5. [foundry](https://book.getfoundry.sh/getting-started/installation)

## Proving Demo

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
    * Starts a local OP Stack devnet using docker.
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
    * Launches the Kailua Bridge RPC.
    * This provides utility RPC methods for bridges to initiate withdrawals.
    * Listens on http://127.0.0.1:1337 and ws://127.0.0.1:1337 by default.
8. `just devnet-fault`
    * Deploys a single `KailuaGame` instance with a faulty sequencing proposal.
    * Tests the validator's fault proving functionality.
    * Tests the proposer's canonical chain tracking functionality.
9. After you're done:
    * `just devnet-down` to stop the running docker containers.
    * `just devnet-clean` to cleanup the docker volumes.

## Questions, Feedback, and Collaborations

We'd love to hear from you on [Discord][discord] or [Twitter][twitter].

[bonsai access]: https://bonsai.xyz/apply
[cargo-risczero]: https://docs.rs/cargo-risczero
[crates]: https://github.com/risc0/risc0/blob/main/README.md#rust-binaries
[dev-docs]: https://dev.risczero.com
[dev-mode]: https://dev.risczero.com/api/generating-proofs/dev-mode
[discord]: https://discord.gg/risczero
[docs.rs]: https://docs.rs/releases/search?query=risc0
[examples]: https://github.com/risc0/risc0/tree/main/examples
[risc0-build]: https://docs.rs/risc0-build
[risc0-repo]: https://www.github.com/risc0/risc0
[risc0-zkvm]: https://docs.rs/risc0-zkvm
[rustup]: https://rustup.rs
[rust-toolchain]: rust-toolchain.toml
[twitter]: https://twitter.com/risczero
[zkvm-overview]: https://dev.risczero.com/zkvm
[zkhack-iii]: https://www.youtube.com/watch?v=Yg_BGqj_6lg&list=PLcPzhUaCxlCgig7ofeARMPwQ8vbuD6hC5&index=5
[kona]: https://github.com/anton-rs/kona
