# Kailua Proposer

The Kailua proposer agent takes care of publishing your local `op-node`'s view of transaction sequencing to Ethereum in
a format that is compatible with the Kailua ZK fault dispute mechanism.
It also attempts to resolve any finalizeable proposals.

## Usage

Starting the Kailua proposer is straightforward:
```shell
Usage: kailua-cli propose [OPTIONS] --op-node-url <OP_NODE_URL> --op-geth-url <OP_GETH_URL> --eth-rpc-url <ETH_RPC_URL> --beacon-rpc-url <BEACON_RPC_URL>
```

```admonish tip
All the parameters above can be provided as environment variables.
```

### Endpoints
The mandatory arguments specify the endpoints that the proposer should use for sequencing:
* `eth-rpc-url`: The parent chain (Ethereum) endpoint for reading/publishing proposals.
* `beacon-rpc-url`: The DA layer (eth-beacon chain) endpoint for retrieving published proposal data.
* `op-geth-url`: The rollup `op-geth` endpoint to read configuration data from.
* `op-node-url`: The rollup `op-node` endpoint to read sequencing proposals from.

### Cache Directory
The proposer saves data to disk as it tracks on-chain proposals.
This allows it to restart quickly.
* `data-dir`: Optional directory to save data to.
  * If unspecified, a tmp directory is created.

### Kailua Deployment
These arguments manually determine the Kailua contract deployment to use and the termination condition.
* `kailua-game-implementation`: The `KailuaGame` contract address.
* `kailua-anchor-address`: Address of the first proposal to synchronize from.
* `final-l2-block`: The last L2 block number to reach and then stop.

### Telemetry
Telemetry data can be exported to an [OTLP Collector](https://opentelemetry.io/docs/collector/).
* `otlp-collector`: The OTLP collector endpoint.

### Rollup Config
These arguments tell Kailua how to read the rollup configuration.
* `bypass-chain-registry`: This flag forces the rollup configuration to be fetched from `op-node` and `op-geth`.

### Wallet
The proposer requires a funded wallet to be able to publish new sequencing proposals on-chain.
* `proposer-key`: The private key for the proposer wallet.
* `proposer-aws-key-id`: AWS KMS Key ID
* `proposer-google-project-id`: GCP KMS Project ID
* `proposer-google-location`: GCP KMS Location
* `proposer-google-keyring`: GCP KMS Keyring Name
* `proposer-google-key-name`: GCP KMS Key name

```admonish tip
`proposer-key` can be replaced with the corresponding AWS/GCP parameters as described [here](upgrade.md#kms-support).
```

```admonish danger
The Kailua proposer wallet is critical for security.
You must keep your proposer's wallet well funded to guarantee the safety and liveness of your rollup.
```

### Transactions
You can control transaction publication through the following three parameters:
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
