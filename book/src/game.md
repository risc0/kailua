# On-chain Sequencing Proposal
```admonish note
If you've successfully performed fast-track migration, you do not need to follow the steps on this page.
```

In this section you will be integrating the `KailuaGame` contract with your rollup's `DisputeGameFactory`.
This will allow Kailua sequencers to submit new proposals!

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


## Set KailuaGame Implementation

The next step is to update the implementation for the Kailua game type stored in the `DisputeGameFactory` contract to
point towards the `KailuaGame` contract deployed previously.
This can be done as follows using your `owner` wallet:
```shell
cast send [YOUR_DISPUTE_GAME_FACTORY] \
  "setImplementation(uint32, address)" \
  [YOUR_KAILUA_GAME_TYPE] \
  [YOUR_DEPLOYED_GAME_CONTRACT]
```

```admonish success
You have now enabled Kailua sequencing proposals to be published!
```

## Enable Withdrawals (Optional)

To enable your users to perform withdrawals using Kailua sequencing proposals, you will need to call 
`setRespectedGameType` on your `OptimismPortal2` contract using your `guardian` wallet.
```admonish bug
This action may cause your optimism `op-proposer` agent to crash.
However, you will later run the Kailua proposer agent for sequencing anyway.
```

```shell
cast send [YOUR_OPTIMISM_PORTAL] \
  "setRespectedGameType(uint32)" \
  [YOUR_KAILUA_GAME_TYPE]
```

```admonish success
You have now enabled withdrawals using resolved (finalized) Kailua sequencing proposals!
```
