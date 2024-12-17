# On-chain Migration

In order to utilize Kailua, you'll need to deploy the Kailua dispute contracts, and configure your rollup to use them.
This process will require access to your rollup's 'Owner' and 'Guardian' wallets.

```admonish tip
The Kailua CLI has a `deploy` command for automating the L1 transactions required to migrate to Kailua.
If your 'Owner' and 'Guardian' wallets are each controlled by one private key,
the `deploy` command can fast-track your on-chain migration.
```

## Fast-track Migration

```admonish info
The fast-track migration tool is restricted to certain rollup deployment configurations.
As the tool is improved to accommodate more setups, these requirements will be relaxed.
```

### Requirements

1. The "Owner" account must be a "Safe" contract instance controlled by a single private-key controlled wallet (EOA).
2. The "Guardian" account must be a private-key controlled wallet (EOA).
3. You must have access to the raw private keys above.

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
  * If you are deploying a new verifier contract and wish to support fake proofs generated in dev mode (insecure), make sure to set `RISC0_DEV_MODE=1` in your environment before invoking the `deploy` command.
* `challenge-timeout`: The timeout (in seconds) for a sequencing proposal to be contradicted.

#### Ethereum Transactions
The next three parameters are the private keys for the respective parent chain wallets:
* `deployer-key`: Private key for the EOA used to deploy the new Kailua contracts.
* `owner-key`: Private key for the sole EOA controlling the Owner "Safe" contract.
* `guardian-key`: Private key for the EOA used as the "Guardian" of the optimism portal.

#### Withdrawals
The final argument configures withdrawals in your rollup:
* `respect-kailua-proposals`: (if present) will allow withdrawals using sequencing proposals finalized by Kailua.

```admonish tip
Skip this flag if you only wish to test Kailua out with no effect on your users.
```

```admonish bug
Changing the respected game type to Kailua may crash the `op-proposer` provided by optimism.
This should be inconsequential because you'll need to run the Kailua proposer for further sequencing to take place anyway.
```
