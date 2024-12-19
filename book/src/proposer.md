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
  --proposer-key [YOUR_PROPOSER_WALLET_PRIVATE_KEY]
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

```admonish danger
The Kailua proposer wallet is critical for security.
You must keep your proposer's wallet well funded to guarantee the safety and liveness of your rollup.
```

## Proposal Data Availability

By default, Kailua uses the beacon chain to publish blobs that contain the extra data required for proposals.

```admonish info
Alternative DA layers for this process will be supported in the future.
```

```admonish success
Running `kailua-cli propose` should now publish Kailua sequencing proposals for your rollup!
```
