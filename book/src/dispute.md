# On-chain Dispute Resolution

```admonish note
If you've successfully performed fast-track migration, you do not need to follow the steps on this page.
```

Kailua's on-chain dispute mechanism is powered by its own custom contracts that define a novel ZK dispute game.
Each rollup has to deploy its own pair of dispute resolution contracts, and this section will guide you through that
process.

The commands below will be using Foundry's `forge` and `cast` utilities, which you should have installed as part of the
foundry [prerequisite](quickstart.md#prerequisites).

```admonish note
The below foundry commands expect both a parameter that determines the wallet to use and the RPC endpoint of the parent
chain.
You will have to add these two parameters manually to every command below.
For more information, refer to `forge create --help`, `cast call --help`, and `cast send --help`
```

First, change your working directory to `crates/contracts/foundry` for `forge` to work:
```shell
cd crates/contracts/foundry
```

```admonish warning
The parameters used to deploy the contracts below are immutable.
Any changes will require redeployment of both the `KailuaGame` and `KailuaTreasury` contracts, along with a repetition of
the steps in the latter sections.
The same `KailuaTreasury` deployment **should NOT** be reused with multiple `KailuaGame` deployments, unless they were
**never** used to publish a proposal (except for the last `KailuaGame` deployment used).
```

## KailuaTreasury
```solidity
constructor(
  IRiscZeroVerifier _verifierContract,
  bytes32 _imageId,
  bytes32 _configHash,
  uint256 _proposalOutputCount,
  uint256 _outputBlockSpan,
  GameType _gameType,
  OptimismPortal2 _optimismPortal,
  Claim _rootClaim,
  uint64 _l2BlockNumber
)
```

This contract stores the collateral bonds required for sequencers to publish their proposal, and also stores the first
sequencing proposal for Kailua as a fault dispute game in your rollup.

```admonish note
Each published proposal on the L1 will cover `proposalOutputCount × outputBlockSpan` L2 blocks, and require
publication of `proposalOutputCount` 32-byte commitments on the DA layer.
```

## Anchor Point

First, you will need to choose the rollup block number from which Kailua sequencing should start.
Then, you need to query your `op-node` for the `outputRoot` at that block number as follows:
```shell
cast rpc --rpc-url [YOUR_OP_NODE_ADDRESS] \
  "optimism_outputAtBlock" \
  $(cast 2h [YOUR_STARTING_L2_BLOCK_NUMBER])
```

```admonish tip
You can quickly filter through the response for `outputRoot` by piping it to `jq -r .outputRoot`
```

### Deployment

Deployment of this contract is via the command below:
```shell
forge create KailuaTreasury --constructor-args \
  [YOUR_RISC_ZERO_VERIFIER] \
  [YOUR_FPVM_IMAGE_ID] \
  [YOUR_ROLLUP_CONFIG_HASH] \
  [YOUR_PROPOSAL_OUTPUT_COUNT] \
  [YOUR_OUTPUT_BLOCK_SPAN] \
  [YOUR_KAILUA_GAME_TYPE] \
  [YOUR_OPTIMISM_PORTAL] \
  [YOUR_OUTPUT_ROOT_CLAIM] \
  [YOUR_L2_BLOCK_NUMBER]
```

Deploying the contract successfully should yield similar output to the following:
```
Deployer: [YOUR_DEPLOYER_WALLET_ADDRESS]
Deployed to: [YOUR_DEPLOYED_TREASURY_CONTRACT]
Transaction hash: [YOUR_DEPLOYMENT_TRANSACTION_HASH]
```
Take note of the contract address since we'll need it later.


```admonish tip
If your rollup `owner` account is controlled by a `Safe` contract, or some other multi-sig contract, you can use
`cast calldata` to get the necessary input that your wallet contract should forward.
* You can use the [safe cli](https://github.com/safe-global/safe-cli) to issue the necessary `send-custom` commands.
* You can use the [safe gui](https://app.safe.global/home) web app to create the necessary transactions.
```

## KailuaGame
```solidity
constructor(
  IKailuaTreasury _kailuaTreasury,
  IRiscZeroVerifier _verifierContract,
  uint256 _genesisTimeStamp,
  uint256 _l2BlockTime,
  Duration _maxClockDuration
)
```

This contract is used by the optimism `DisputeGameFactory` to instantiate every Kailua sequencing proposal after the
initial one in the `KailuaTreasury`.
Deployment is fairly similar to the treasury via the command below:

```shell
forge create KailuaGame --evm-version cancun --constructor-args \
  [YOUR_DEPLOYED_TREASURY_CONTRACT] \
  [YOUR_GENESIS_TIMESTAMP] \
  [YOUR_BLOCK_TIME] \
  [YOUR_MAX_CLOCK_DURATION]
```

```admonish note
The above forge command requires the `--evm-version cancun` argument.
```

Deploying the contract successfully should yield similar output to the following:
```
Deployer: [YOUR_DEPLOYER_WALLET_ADDRESS]
Deployed to: [YOUR_DEPLOYED_GAME_CONTRACT]
Transaction hash: [YOUR_DEPLOYMENT_TRANSACTION_HASH]
```
Note down this contract's address, we'll use it later.
There is no configuration needed for this contract.

```admonish success
You now have two Kailua dispute resolution contracts tailored to your rollup and ZK verifier!
```
