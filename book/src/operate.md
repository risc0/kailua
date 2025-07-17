# Off-chain Agents

Once your chain contracts are upgraded to integrate Kailua, this section describes what you need to do for sequencing,
withdrawals, and fault proving to take place in your rollup using Kailua.
Kailua provides two agents to take on the role of the standard Optimism `op-proposer` and `op-challenger` agents, and
one RPC server to facilitate dispute game contract selection during withdrawals.

```admonish danger
Without a Vanguard, sequencing in Kailua is fully permissionless at all time.
Anyone can run these Kailua agents for your rollup and publish proposals when possible.
```

```admonish warning
Just like their optimism counterparts, the Kailua Proposer and Validator must remain online and their wallets 
sufficiently funded to guarantee the safety and liveness of your rollup.
```

```admonish info
When using the `proveWithdrawalTransaction`/`finalizeWithdrawalTransaction` functions in `OptimismPortal2` with Kailua
games, you cannot just use any sequencing proposal that has a valid root claim and assume it can be eventually resolved.
The Kailua RPC can be used to query which dispute game contract can be used to initiate a prove a withdrawal transaction.
```
