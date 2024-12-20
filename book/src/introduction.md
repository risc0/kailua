# Kailua

[![Static Badge](https://img.shields.io/badge/GitHub-kailua-green?logo=github)](https://github.com/risc0/kailua)

This document is a guide to the design, implementation, and usage of RISC Zero's Kailua.

```admonish note
Kailua is in active development and has not yet been reviewed for use in production.
```

## Introduction

Kailua is suite of tools and contracts for upgrading Optimistic rollups to use ZK Fault Proofs powered by the RISC Zero zkVM.
Kailua introduces its own novel fault proof game design which provides the best in class security guarantees for sequencing rollup transactions.
These benefits come at marginal added operational costs compared to full validity proving.

### Withdrawal Delay

A delay attack happens when a dishonest party attempts to delay the withdrawal finality of correctly sequenced transactions.
For optimistic rollups, the main attack vector is through triggering on-chain disputes using the fault proving mechanism.

```admonish check
Kailua's dispute resolution mechanism resolves disputes as fast as proofs can be generated.
Thanks to the RISC Zero zkVM's scale-out design, this means that the impact of delay attacks can be mitigated with
more proving power.
For example, the worst-case single-block dispute requires proving 100bn cycles in the zkVM, a workload that can be
computed by RISC Zero's Bonsai service in under an hour.
```

### Denial-of-Service

Assuming an honest majority operates the parent chain of the rollup, on-chain denial-of-service attacks can still
happen if a wealthy party raises the on-chain gas costs beyond what honest participants in the fault proof protocol can
afford.
This block congestion attack can effectively censor disputes against faulty sequencing proposals from being made
on-chain, threatening the safety of the rollup.

```admonish check
Kailua's design incorporates "Adaptive Dispute Cutoffs", which delays withdrawal finality to increase the dispute 
opportunity based on the level of on-chain congestion. This guarantees that if faults cost more to dispute than a
predetermined amount, honest parties will be granted more time until gas costs subside.
```

### Sybil Identities

**Whale** attackers can overwhelm honest parties in a dispute resolution mechanism by using multiple identities to flood
the system with disputes.
In fault proving schemes where a defender has to issue a timely response on-chain to every dispute, the costs borne
by the defender to continuously participate in all open disputes until they are resolved can be overwhelming, leading
to some faults slipping through.

```admonish check
Sybil attacks against Kailua force attackers to prove each other's faults at no added cost to the honest defender.
The only requirement for safety in Kailua is for an honest party to submit a correct sequencing proposal.
The added requirement for liveness is for disputes to be resolved through proofs, which carry no time limit to
generate.
```

### Resource Exhaustion

Some fault proof protocols require additional collateral to be staked for every move made in the system, while others
require proofs to be generated in a timely manner.
These two requirements cause some other systems to be vulnerable to resource exhaustion, where the resource can be
the collateral or the proving power required for an honest party to issue a timely response, even if it can afford the
transaction fees.

```admonish check
Kailua operates under constant collateral requirements for honest parties, and places no restrictions on proving
times, enabling honest parties to successfully defend against attacks of any size at a pre-determined maximum cost.
```