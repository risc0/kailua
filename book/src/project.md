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

The CLI for Kailua is designed to support four main commands:
* `Upgrade`: Upgrades an existing rollup deployment to utilize Kailua for fault proving.
* `Propose`: Monitor a rollup for sequencing state and publish proposals on-chain (akin to op-proposer).
* `Validate`: Monitor a rollup for disputes and publish the necessary FPVM proofs for resolution.
* `Fault`: Submit garbage proposals to test fault proving.

## Contracts

The contracts directory is a foundry project comprised of the following main contracts:
* `KailuaTournament.sol`: Logic for resolving disputes between contradictory proposals.
* `KailuaTreasury.sol`: Logic for maintaining collateral and paying out provers for resolving disputes.
* `KailuaGame.sol`: Logic for introducing new sequencing proposals.
* `KailuaLib.sol`: Misc. utilities.

## FPVM

Kailua executes Optimism's `Kona` inside the RISC Zero zkVM to create fault proofs.
The following project components enable this:
* `bin/host`: A modified version of `Kona`'s host binary, which acts as an oracle for the witness data required to create a fault proof.
* `bin/client`: A modified version of `Kona`'s client binary, which executes the `fpvm` while querying the host for the necessary chain data.
* `build/risczero/fpvm`: The zkVM wrapper around `Kona` to create ZK fault proofs.
