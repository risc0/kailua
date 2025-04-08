# Parameters

Before migrating to Kailua, you'll need to decide on a few setup parameters and note them down for later use during
the migration process.

## Starting Block Number
You'll need to pick a block number for Kailua to start sequencing from.
The sequencing state according to your `op-node` at the block you pick will be immediately finalized.
When you choose to enable withdrawal against Kailua sequencing proposals, your users will be able to start withdrawals
using this finalized state.

```admonish tip
You can postpone enabling withdrawals using Kailua at any later point in time after successful migration.
```

## Proposal Output Count / Output Block Span
Each sequencing proposal in Kailua must cover a fixed number of L2 blocks, which will determine how much data must be
published per proposal.
Consequently, configuring how many output commitments are published per proposal and how many L2 blocks are covered per
commitment will determine your proposer's DA costs for using Kailua and your validator's proving costs during dispute.

These commitments are published as Blobs, which means you should optimize your block span `S` to be `S = B * 4096 + 1`,
where `B` is the number of blobs required for a single proposal transaction (Ethereum currently limits a single block to
at most 6 blobs, i.e. `B < 7`).

Subsequently, combining `S` with your rollup's block time determines how often your sequencer has to publish a proposal
to ensure the liveness of your chain.

```admonish example
Consider Optimism Mainnnet as an example, which has a block time of 2 seconds.
To keep its current average sequencing frequency of ~55 minutes, it only needs to publish ~1650 commitments per proposal.
To maximize the utilization of the extra blob published when proposing, OP Mainnet can relax its proposal rate to once
per 2 hours and 15 minutes.
```

## Proposal Time Gap

```admonish tip
This value can be safely set to zero when relying only on validity proving using a very large dispute timeout.
```

Because Kailua is designed for permissionless sequencing, it has an extra safety mechanism that can prevent sequencing 
proposals from being made eagerly before the parent chain data supporting that state is finalized.

This mechanism comes in the form of a forced delay between the timestamp of the L2 block being proposed, and the current
timestamp on the parent chain (ethereum).

While the Kailua proposer agent won't publish a sequencing proposal until it is considered safe by op-node, the Kailua contracts
allow you to enforce this requirement so that even an eager (potentially dishonest) proposer cannot have a head start!

```admonish warning
The combined duration of the time gap and the challenge timeout should not be less than twice the length of your rollup's sequencing window.
```

```admonish example
Consider a Rollup on Ethereum with a 50 block sequencing window.
At the time of writing, the Ethereum block time is 12 seconds.
Consequently, the minimum proposal time gap value should be `50 × 12 = 600` seconds (10 minutes).
```

```admonish note
At the time of writing, Ethereum finalizes each block in [approximately 15 minutes](https://ethereum.org/en/roadmap/single-slot-finality/#:~:text=It%20takes%20about%2015%20minutes%20for%20an%20Ethereum%20block%20to%20finalize).
Consequently, we recommend you add an extra `15 × 60 = 900` seconds to your gap to match.
```



## Collateral Amount

The collateral requirements for a sequencer in Kailua come in the form of a fixed amount to be deposited, independent of
how many sequencing proposals are in flight.
This is because a malicious Kailua proposer, and any faulty sequencing proposals it has published, is eliminated using
only a single fault proof.

The prover who submitted that fault proof consequently gets compensated with the faulty sequencer's staked collateral.
This collateral should at least cover the proving cost, but should also include a sizeable tip for the prover to
incentivize proving priority.
Our estimates put a worst-case proving cost using Bonsai for a single (OP Mainnet) block at $100 USD.

```admonish example
Currently, OP Mainnet requires 0.08 ETH (\~$300) of collateral per proposal, and finalizes a proposal after at least 3.5
days if it is undisputed.
This means, at an average hourly rate of proposing, the proposer has `84 * 0.08 = 6.72` ETH (\~$3700 USD) on average 
locked up as collateral in the best case where no disputes take place.

Using Kailua, 0.08 ETH would be sufficient as the total collateral locked up by the proposer, even under the same finality
delay.
This would cover the worst-case proving cost in case of dispute, and, discounting transaction costs, leave a $200 tip.
```

## Challenge Timeout

The current implementation of Kailua does not yet have adaptive dispute periods based on congestion.
Consequently, you should keep your existing challenge timeout period.

```admonish note
If you wish to practically operate Kailua using only validity proofs, set this value to `31536000000000000` seconds
(i.e. 1 billion calendar years).
```

```admonish warning
The combined duration of the time gap and the challenge timeout should not be less than twice the length of your rollup's sequencing window.
```

## Verifier Contract
RISC Zero maintains a set of pre-deployed verifier contracts for its ZK proving system.
These contracts are regularly upgraded to support new releases of the prover, and also have a permissionless fail-safe
mechanism that anyone who can produce a proof-of-exploit can trigger to halt the verifier.

```admonish note
You must ensure that the chosen verifier contract supports your RISC Zero zkVM version.
Once a new zkVM version is released, there can be a delay in adding it to the router.
```

You have the choice of either using the already deployed verifier for your parent chain, or deploying and maintaining
your own verifier contracts, as described in the later sections.

## Vanguard Advantage
Kailua supports the designation of one proposer as a *Vanguard*, along with an *advantage time* for said proposer.
When set, this designation prevents any other proposer from publishing a proposal for a certain block height until
either the Vanguard has made a proposal for that height, or the advantage time granted to the Vanguard has passed.
The advantage time is counted down until after the Vanguard was allowed to publish a proposal.
The Vanguard still needs to lock up the necessary bond to make proposals.

```admonish note
Having a Vanguard that makes honest proposals within the advantage time guarantees the safety of the rollup.
```

```admonish warning
If the advantage time is unreasonably long, Vanguard downtime delays the liveness of the rollup.
```

This middleground enables rollups transitioning from a permissioned scheme to enjoy permissionless fault proofs in
Kailua with an added safety net.

```admonish success
Once you've got your parameters all planned out, you're ready for the next step!
```
