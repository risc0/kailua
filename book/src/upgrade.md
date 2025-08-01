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
   * (Optional) Designate a Vanguard proposer.
   * (Optional) Enable withdrawals using finalized Kailua proposals.

```admonish tip
The Kailua CLI has a `fast-track` command for automating the L1 transactions required to migrate to Kailua.
If the command does not yet support your configuration, you'll need to follow the manual steps in the next sub-sections.
```

```admonish hint
You might find it useful to rehearse migration using a local devnet first.
```

### Estimated Gas Costs

The below table contains rounded-up gas cost estimates of various contract operations

| Contract                | Operation        |    Gas | Notes                  |
|:------------------------|:-----------------|-------:|------------------------|
| RiscZeroVerifierRouter  | deploy           |   800K |                        |
| RiscZeroVerifierRouter  | addVerifier      |    50K |                        |
| RiscZeroGroth16Verifier | deploy           | 1,300K |                        |
| RiscZeroMockVerifier    | deploy           |   615K |                        |
| KailuaTreasury          | deploy           | 5,200K |                        |
| KailuaTreasury          | propose          |   400K | 1 blob                 |
| KailuaGame              | deploy           | 5,000K |                        |
| KailuaGame              | proveValidity    |   375K | Groth16 proof          |
| KailuaGame              | proveOutputFault |   415K | Groth16 + 1 KZG proofs |
| KailuaGame              | proveOutputFault |   470K | Groth16 + 2 KZG proofs |
| KailuaGame              | proveNullFault   |   171K | 1 KZG proof            |
| KailuaGame              | resolve          |   120K | Undisputed             |
| KailuaGame              | resolve          |   160K | 1 fault                |
| KailuaGame              | resolve          |   280K | 2 faults               |
| KailuaGame              | resolve          |   370K | 3 faults               |


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
      --proposal-output-count [YOUR_OUTPUTS_PER_PROPOSAL] \
      --output-block-span [YOUR_BLOCKS_PER_OUTPUT] \
\
      --collateral-amount [YOUR_COLLATERAL_AMOUNT] \
      --verifier-contract [RISC_ZERO_VERIFIER_ADDRESS] \
      --challenge-timeout [YOUR_CHALLENGE_PERIOD] \
\
      --deployer-key [YOUR_DEPLOYER_KEY] \
      --owner-key [YOUR_OWNER_KEY] \
      --guardian-key [YOUR_GUARDIAN_KEY] \
\
      --vanguard-address [YOUR_VANGUARD_ADDRESS] \
      --vanguard-advantage [YOUR_VANGUARD_ADVANTAGE] \
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
* `proposal-output-count`: The number of intermediate output commitments published per proposal.
* `output-block-span`: The number of rollup blocks each intermediate output commitment must cover.

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

##### KMS Support
Instead of raw private keys, you can use either AWS or GCP for obtaining transaction signatures from a KMS.
* AWS: Specify a corresponding `[EOA]-aws-key-id` parameter. The remainder of the [AWS configuration](https://docs.aws.amazon.com/sdkref/latest/guide/creds-config-files.html) is handled by the AWS SDK.
  * Example: `deployer-aws-key-id`.
* GCP: Specify the corresponding `[EOA]-google-project-id`, `[EOA]-google-location`, `[EOA]-google-keyring` and `[EOA]-google-key-name` parameters.
  * Example: `owner-google-project-id`, `owner-google-location`, `owner-google-keyring` and `owner-google-key-name`.


#### Vanguard Proposer
The next two (optional) parameters define the Vanguard proposer advantage:
* `vanguard-address`: The address of the designated Vanguard.
* `vanguard-advantage`: The amount of time (in seconds) to grant proposal exclusivity to the Vanguard.
  * If unspecified, this defaults to `1152921504606846975` (a practically indefinite advantage of 36 Billion years).

#### Withdrawals
```admonish bug
Changing the respected game type to Kailua may crash the `op-proposer` provided by optimism depending on its version.
This should be inconsequential because you'll need to run the Kailua proposer for further sequencing to take place anyway.
```

The final argument configures withdrawals in your rollup:
* `respect-kailua-proposals`: (if present) will allow withdrawals using sequencing proposals finalized by Kailua.

```admonish done
If you've successfully completed fast-track migration using the tool, you may now skip to the [Off-chain page](./operate.md).
```