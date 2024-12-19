# On-chain State Anchoring

```admonish note
If you've successfully performed fast-track migration, you do not need to follow the steps on this page.
```

In this section you will be integrating the `KailuaTreasury` contract with your rollup's `DisputeGameFactory`.
This will finalize the initial sequencing proposal from which Kailua will start.

The commands below will be using Foundry's `cast` utility, which you should have installed as part of the
foundry [prerequisite](quickstart.md#prerequisites).

```admonish note
The below foundry commands expect both a parameter that determines the wallet to use and the rpc endpoint of the parent
chain.
You will have to add these two parameters manually to every command below.
For more information, refer to `cast call --help`, and `cast send --help`
```

```admonish tip
If your rollup `owner` account is controlled by a `Safe` contract, or some other multi-sig contract, you can use
`cast calldata` to get the necessary input that your wallet contract should forward.
```

## Clear DGF Kailua Bond
Optimism's `DisputeGameFactory` is design to require a bond value for each sequencing proposal.
The `KailuaTreasury` instead requires a constant bond value for a sequencer to make any number of proposals.
To ensure that the Kailua sequencer operates as expected, we will need to set this value to zero for Kailua proposals
if it is non-zero.
You can check the value as follows:

```shell
cast call [YOUR_DISPUTE_GAME_FACTORY] \
  "initBonds(uint32) returns (uint256)" \
  [YOUR_KAILUA_GAME_TYPE]
```

If the returned value is non-zero, you must reset it through `setInitBond` using your rollup `owner` wallet:

```shell
cast send [YOUR_DISPUTE_GAME_FACTORY] \
  "setInitBond(uint32, uint256)" \
  [YOUR_KAILUA_GAME_TYPE] \
  0
```

## Set KailuaTreasury Implementation

The next step is to update the implementation for the Kailua game type stored in the `DisputeGameFactory` contract to
point towards the `KailuaTreasury` contract deployed in the last section.
This can be done as follows using your `owner` wallet:
```shell
cast send [YOUR_DISPUTE_GAME_FACTORY] \
  "setImplementation(uint32, address)" \
  [YOUR_KAILUA_GAME_TYPE] \
  [YOUR_DEPLOYED_TREASURY_CONTRACT]
```

## Anchor Instantiation

Once the implementation is set, the next step is to create a dispute game instance using the treasury.
This step is only to be done once in order to create a starting point for sequencing using Kailua.
```admonish warning
This step will publish and immediately resolve (finalize) a single sequencing proposal with no chance for dispute.
```

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

Once you have the `outputRoot` value you wish to start sequencing from, the next step is to call `create` on `DisputeGameFactory` using the `owner` wallet:
```shell
cast send [YOUR_DISPUTE_GAME_FACTORY] \
  "create(uint32, bytes32, bytes)" \
  [YOUR_KAILUA_GAME_TYPE] \
  [YOUR_OUTPUT_ROOT] \
  $(cast abi-encode --packed "f(uint64)" [YOUR_STARTING_L2_BLOCK_NUMBER])
```

```admonish note
The above cast abi-encode command requires the `--packed` argument.
```


To get the address of this new game instance, use the `games` function on the `DisputeGameFactory`:
```shell
cast call [YOUR_DISPUTE_GAME_FACTORY] \
  "games(uint32, bytes32, bytes) returns (address, uint64)" \
  [YOUR_KAILUA_GAME_TYPE] \
  [YOUR_OUTPUT_ROOT] \
  $(cast abi-encode --packed "f(uint64)" [YOUR_STARTING_L2_BLOCK_NUMBER])
```

With this instance address, the last step is to call `resolve()` on it using the `owner` wallet:
```shell
cast send [YOUR_GAME_INSTANCE_ADDRESS] \
  "resolve()"
```

```admonish success
You have now set a sequencing starting point for Kailua!
```