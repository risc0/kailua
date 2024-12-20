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
The below foundry commands expect both a parameter that determines the wallet to use and the rpc endpoint of the parent
chain.
You will have to add these two parameters manually to every command below.
For more information, refer to `forge create --help`, `cast call --help`, and `cast send --help`
```

First, change your working directory to `crates/contracts/foundry` for `forge` to work:
```shell
cd crates/contracts/foundry
```

## KailuaTreasury
```solidity
constructor(
  IRiscZeroVerifier _verifierContract,
  bytes32 _imageId,
  bytes32 _configHash,
  uint256 _proposalBlockCount,
  GameType _gameType,
  IDisputeGameFactory _disputeGameFactory
)
```

This contract stores the collateral bonds required for sequencers to publish their proposal, and also stores the first
sequencing proposal for Kailua as a fault dispute game in your rollup.

### Deployment

Deployment of this contract is via the command below:
```shell
forge create KailuaTreasury --constructor-args \
  [YOUR_RISC_ZERO_VERIFIER] \
  [YOUR_FPVM_IMAGE_ID] \
  [YOUR_ROLLUP_CONFIG_HASH] \
  [YOUR_PROPOSAL_BLOCK_COUNT] \
  [YOUR_KAILUA_GAME_TYPE] \
  [YOUR_DISPUTE_GAME_FACTORY]
```

Deploying the contract successfully should yield similar output to the following:
```
Deployer: [YOUR_DEPLOYER_WALLET_ADDRESS]
Deployed to: [YOUR_DEPLOYED_TREASURY_CONTRACT]
Transaction hash: [YOUR_DEPLOYMENT_TRANSACTION_HASH]
```
Take note of the contract address since we'll need it later.

### Configuration

Once deployed, you'll need to set the bond value (in wei) required for sequencers.
This is done by calling the `setParticipationBond` function on the treasury contract using the `owner` wallet for your
rollup.

For example, if your bond value is 12 eth, first convert this to wei using `cast`:
```shell
cast to-wei 12
```
```
12000000000000000000
```
Then, configure the bond as follows using the rollup `owner` wallet:
```shell
cast send \
  [YOUR_DEPLOYED_TREASURY_CONTRACT] \
  "setParticipationBond(uint256 amount)" \
  12000000000000000000
```

```admonish tip
If your rollup `owner` account is controlled by a `Safe` contract, or some other multi-sig contract, you can use
`cast calldata` to get the necessary input that your wallet contract should forward.
* You can use the [safe-cli](https://github.com/safe-global/safe-cli) to issue the necessary `send-custom` commands.
  * `docker run -it safeglobal/safe-cli safe-cli <safe> <rpc>`
* You can use the [safe](https://app.safe.global/home) web app to create the necessary transactions.
```

## KailuaGame
```solidity
constructor(
  IKailuaTreasury _kailuaTreasury,
  IRiscZeroVerifier _verifierContract,
  bytes32 _imageId,
  bytes32 _configHash,
  uint256 _proposalBlockCount,
  GameType _gameType,
  IDisputeGameFactory _disputeGameFactory,
  uint256 _genesisTimeStamp,
  uint256 _l2BlockTime,
  uint256 _proposalTimeGap,
  Duration _maxClockDuration
)
```

This contract is used by the optimism `DisputeGameFactory` to instantiate every Kailua sequencing proposal after the
initial one in the `KailuaTreasury`.
Deployment is fairly similar to the treasury via the command below:

```shell
forge create KailuaGame --evm-version cancun --constructor-args \
  [YOUR_DEPLOYED_TREASURY_CONTRACT] \
  [YOUR_RISC_ZERO_VERIFIER] \
  [YOUR_FPVM_IMAGE_ID] \
  [YOUR_ROLLUP_CONFIG_HASH] \
  [YOUR_PROPOSAL_BLOCK_COUNT] \
  [YOUR_KAILUA_GAME_TYPE] \
  [YOUR_DISPUTE_GAME_FACTORY] \
  [YOUR_GENESIS_TIMESTAMP] \
  [YOUR_BLOCK_TIME] \
  [YOUR_PROPOSAL_TIME_GAP] \
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
