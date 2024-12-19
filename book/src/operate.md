# Off-chain Agents

Once your chain contracts are upgraded to integrate Kailua, this section describes what you need to do for sequencing
and fault proving to take place in your rollup using Kailua.
Kailua provides two agents to take on the role of the standard Optimism `op-proposer` and `op-challenger` agents.

```admonish tip
If you will only be using Kailua for sequencing (and withdrawals), you should terminate your `op-proposer` and
`op-challenger` agent processes if you plan on reusing their wallets to avoid transaction errors.
```

```admonish warning
Just like their optimism counterparts, the Kailua agents must remain online and their wallets sufficiently funded to 
guarantee the safety and liveness of your rollup.
```

```admonish danger
Kailua currently only supports permissionless sequencing.
This means that anyone can run these Kailua agents locally for your rollup.
```
