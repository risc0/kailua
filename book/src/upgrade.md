# On-chain Contracts

In order to utilize Kailua, you'll need to deploy the Kailua dispute contracts, and configure your rollup to use them.
This process will require access to your rollup's 'Owner' and 'Guardian' wallets.

## Overview

The steps required to upgrade your on-chain rollup contracts to support Kailua are as follows:
1. (Optional) Deploy a RISC Zero Verifier contract set.
   * This can be skipped by using a pre-existing verifier contract.
2. Deploy a `KailuaTreasury` and a `KailuaGame` contract with your configuration.
3. Initialize the `KailuaTreasury` contract to mark the start of sequencing under Kailua.
4. Update the rollup's `DisputeGameFactory` contract to use `KailuaGame` for sequencing proposals.
   * (Optional) Enable withdrawals using finalized Kailua proposals.

```admonish tip
The Kailua CLI has a `fast-track` command for automating the L1 transactions required to migrate to Kailua.
If the command does not yet support your configuration, you'll need to follow the manual steps in the next sub-sections.
```

```admonish hint
You might find it useful to rehearse migration using a local devnet first.
```

## Fast-track Migration

```admonish info
The fast-track migration tool is restricted to certain rollup deployment configurations.
As the tool is improved to accommodate more setups, these requirements will be relaxed.
```

### Requirements

1. The "Owner" account must be a "Safe" contract instance controlled by a single private-key controlled wallet (EOA).
2. The "Guardian" account must be a private-key controlled wallet (EOA).
3. You must have access to the raw private key(s) above.

```admonish tip
You can skip the guardian key/account requirements if you do not wish to enable withdrawals against sequencing proposals
made by Kailua as part of the fast-track process via the `respect-kailua-proposals` flag.
You can enable withdrawals manually later using the `OptimismPortal2` contract.
```

### Usage

If all the above conditions are met, you can fast track the migration of your rollup to Kailua as follows:

```shell
kailua-cli fast-track \
      --eth-rpc-url [YOUR_ETH_RPC_URL] \
      --op-geth-url [YOUR_OP_GETH_URL] \
      --op-node-url [YOUR_OP_NODE_URL] \
\
      --starting-block-number [YOUR_STARTING_BLOCK_NUMBER] \
      --proposal-block-span [YOUR_BLOCKS_PER_PROPOSAL] \
      --proposal-time-gap [YOUR_PROPOSAL_TIME_GAP] \
\
      --collateral-amount [YOUR_COLLATERAL_AMOUNT] \
      --verifier-contract [RISC_ZERO_VERIFIER_ADDRESS] \
      --challenge-timeout [YOUR_CHALLENGE_PERIOD] \
\
      --deployer-key [YOUR_DEPLOYER_KEY] \
      --owner-key [YOUR_OWNER_KEY] \
      --guardian-key [YOUR_GUARDIAN_KEY] \
\
      --respect-kailua-proposals
```
```admonish tip
All the parameters above can be provided as environment variables.
```

#### Endpoints
The first three parameters to this command are the L1 and L2 RPC endpoints:
* `eth-rpc-url`: The endpoint for the parent chain.
* `op-geth-url`: The endpoint for the rollup execution client.
* `op-node-url`: The endpoint for the rollup consensus client.

#### Sequencing
The next three parameters configure sequencing:
* `starting-block-number`: The rollup block number to immediately finalize and start sequencing from.
* `proposal-block-span`: The number of rollup blocks each sequencing proposal must cover.
* `proposal-time-gap`: The minimum amount of time (in seconds) that must pass before a rollup block can be sequenced.

```admonish warning
The sequencing state at the block `starting-block-number` as reported by the `op-node` will be finalized without delay.
```

#### Fault Proving
The next three parameters configure fault proving:
* `collateral-amount`: The amount of collateral (in wei) a sequencer has to stake before publishing proposals.
* `verifier-contract`: (Optional) The address of the existing RISC Zero verifier contract to use. If this argument is omitted, a new set of verifier contracts will be deployed.
  * If you wish to use an already existing verifier, you must provide this argument, even if the `config` command had located a verifier.
  * If you are deploying a new verifier contract and wish to support fake proofs generated in dev mode (insecure), make sure to set `RISC0_DEV_MODE=1` in your environment before invoking the `fast-track` command.
* `challenge-timeout`: The timeout (in seconds) for a sequencing proposal to be contradicted.

#### Ethereum Transactions
The next three parameters are the private keys for the respective parent chain wallets:
* `deployer-key`: Private key for the EOA used to deploy the new Kailua contracts.
* `owner-key`: Private key for the sole EOA controlling the Owner "Safe" contract.
* `guardian-key`: Private key for the EOA used as the "Guardian" of the optimism portal.

#### Withdrawals
```admonish bug
Changing the respected game type to Kailua may crash the `op-proposer` provided by optimism.
This should be inconsequential because you'll need to run the Kailua proposer for further sequencing to take place anyway.
```

The final argument configures withdrawals in your rollup:
* `respect-kailua-proposals`: (if present) will allow withdrawals using sequencing proposals finalized by Kailua.

```admonish done
If you've successfully completed fast-track migration using the tool, you may now skip to the [Off-chain page](./operate.md).
```