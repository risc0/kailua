# Project

Kailua's project structure is primarily as follows:

```
kailua                      // Root project directory
├── bin                     
│   ├── cli                 // Main Kailua CLI
│   ├── client              // FPVM Client
│   └── host                // FPVM Host
├── book                    // This document
├── build                   
│   └── risczero            // RISC Zero zkVM proving backend
├── crates                  
│   ├── common              // Fault proving primitives
│   └── contracts           // Fault proof contracts
├── justfile                // Convenience commands
└── testdata
    └── 16491249            // Example FPVM test data for op-sepolia block
```

## CLI

The CLI for Kailua is designed to support five main commands:
* `config`: Outputs configuration information required for migration.
* `fast-track`: Automatically upgrades an existing rollup deployment to utilize Kailua for fault proving.
* `propose`: Monitor a rollup for sequencing state and publish proposals on-chain (akin to op-proposer).
* `validate`: Monitor a rollup for disputes and publish the necessary FPVM proofs for resolution.
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
* `bin/host`: A modified version of `Kona`'s host binary, which acts as an oracle for the witness data required to create a fault proof.
* `bin/client`: A modified version of `Kona`'s client binary, which executes the `fpvm` while querying the host for the necessary chain data.
* `build/risczero/fpvm`: The zkVM binary to create ZK fault proofs with `Kona`.
* `crates/common`: A wrapper crate around `Kona` with utilities for efficient ZK fault proving.
