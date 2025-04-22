# Kailua Proposer

The Kailua proposer agent takes care of publishing your local `op-node`'s view of transaction sequencing to Ethereum in
a format that is compatible with the Kailua ZK fault dispute mechanism.
It also attempts to resolve any finalizeable proposals.

## Usage

Starting the Kailua proposer is straightforward:
```shell
kailua-cli propose \
  --eth-rpc-url [YOUR_ETH_RPC_URL] \
  --beacon-rpc-url [YOUR_BEACON_RPC_URL] \
  --op-geth-url [YOUR_OP_GETH_URL] \
  --op-node-url [YOUR_OP_NODE_URL] \
  --data-dir [YOUR_PROPOSER_DATA_CACHE_PATH] \
  --proposer-key [YOUR_PROPOSER_WALLET_PRIVATE_KEY] \
  --txn-timeout [YOUR_TRANSACTION_TIMEOUT_SECONDS] \
  --exec-gas-premium [YOUR_EXECUTION_GAS_PREMIUM_PERCENTAGE] \
  --blob-gas-premium [YOUR_BLOB_GAS_PREMIUM_PERCENTAGE]
```

```admonish tip
All the parameters above can be provided as environment variables.
```

### Endpoints
The first four arguments specify the endpoints that the proposer should use for sequencing:
* `eth-rpc-url`: The parent chain (ethereum) endpoint for reading/publishing proposals.
* `beacon-rpc-url`: The DA layer (eth-beacon chain) endpoint for retrieving published proposal data.
* `op-geth-url`: The rollup `op-geth` endpoint to read configuration data from.
* `op-node-url`: The rollup `op-node` endpoint to read sequencing proposals from.

### Cache Directory (Optional)
The proposer saves data to disk as it tracks on-chain proposals.
This allows it to restart quickly without requesting a lot of old on-chain data if terminated.
* `data-dir`: Optional directory to save data to.
  * If unspecified, a tmp directory is created.

### Wallet
The proposer requires a funded wallet to be able to publish new sequencing proposals on-chain.
* `proposer-key`: The private key for the proposer wallet.

```admonish tip
`proposer-key` can be replaced with the corresponding AWS/GCP parameters as described [here](upgrade.md#kms-support).
```

```admonish danger
The Kailua proposer wallet is critical for security.
You must keep your proposer's wallet well funded to guarantee the safety and liveness of your rollup.
```

### Transactions
You can control transaction publication through the three following parameters:
* `txn-timeout`: A timeout in seconds for transaction broadcast (default 120)
* `exec-gas-premium`: An added premium percentage to estimated execution gas fees (Default 25)
* `blob-gas-premium`: An added premium percentage to estimated blob gas fees (Default 25).

The premium parameters increase the internally estimated fees by the specified percentage.

### Upgrades
If you re-deploy the KailuaTreasury/KailuaGame contracts to upgrade your fault proof system, you will need to restart
your proposer (and validator).
By default, the proposer (and validator) will use the latest contract deployment available upon start up, and ignore any
proposals not made using them.
If you wish to start a proposer for a past deployment, you can explicitly specify the deployed KailuaGame contract
address using the optional `kailua-game-implementation` parameter.
```admonish note
When running on an older deployment, the proposer will not create any new proposals, but will finalize any old ones once
possible.
```


## Proposal Data Availability

By default, Kailua uses the beacon chain to publish blobs that contain the extra data required for proposals.

```admonish info
Alternative DA layers for this process will be supported in the future.
```

```admonish success
Running `kailua-cli propose` should now publish Kailua sequencing proposals for your rollup!
```
