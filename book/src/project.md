# Project

Kailua's project structure is primarily as follows:

```
kailua                      // Root project directory
├── bin                     
│   └── cli                 // Main Kailua CLI
├── book                    // This document
├── build                   
│   └── risczero            // RISC Zero zkVM proving backend
├── crates                  
│   ├── common              // Fault proving primitives
│   ├── contracts           // Fault proof contracts
│   ├── proposer            // Sequencing proposal submitter
│   ├── prover              // Proof generation orcherstrator
│   ├── sync                // Sequencing proposal tracker
│   └── validator           // Sequencing proposal validator
├── justfile                // Convenience commands
└── testdata
    └── 16491249            // Example FPVM test data for op-sepolia block
```

## CLI

The CLI for Kailua is designed to support seven commands:
* `config`: Outputs configuration information required for migration.
* `demo`: Automatically generate validity proofs for any running L2 chain.
* `fast-track`: Automatically upgrades an existing rollup deployment to utilize Kailua for fault proving.
* `propose`: Monitor a rollup for sequencing state and publish proposals on-chain (akin to op-proposer).
* `validate`: Monitor a rollup for disputes and publish the necessary FPVM proofs for resolution.
* `benchmark`: Generates proofs for performance benchmarking.
* `fault`: Submit garbage proposals to test fault proving.

## Contracts

The contracts directory is a foundry project comprised of the following main contracts:
* `KailuaTournament.sol`: Logic for resolving disputes between contradictory proposals.
* `KailuaTreasury.sol`: Logic for maintaining collateral and paying out provers for resolving disputes.
* `KailuaGame.sol`: Logic for introducing new sequencing proposals.
* `KailuaLib.sol`: Misc. utilities.

The `kailua-contracts` crate builds and exports these contracts in Rust.

## FPVM

The Kailua FPVM executes Optimism's `Kona` inside the RISC Zero zkVM to derive and execute optimism blocks and create fault proofs.
The following project components work together to enable this functionality:
* `build/risczero/fpvm`: The zkVM binary to create ZK fault proofs with `Kona`.
* `crates/common`: A wrapper crate around `Kona` with utilities for efficient ZK fault proving.
* `crates/prover`: An orchestrator for proof generation locally, remotely on Bonsai, or through Boundless.
