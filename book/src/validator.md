# Kailua Validator

The Kailua validator watches your rollup for sequencing proposals that contradict each other and generates a ZK fault
proof to settle the dispute between them.

```admonish note
The Kailua validator agent requires access to an archive `op-geth` rollup node to retrieve data during proof generation.
Node software other than `op-geth` is not as reliable for the necessary `debug` namespace rpc calls.
```

## Usage

Starting the Kailua validator is straightforward:
```shell
kailua-cli validate [OPTIONS] --op-node-url <OP_NODE_URL> --op-geth-url <OP_GETH_URL> --eth-rpc-url <ETH_RPC_URL> --beacon-rpc-url <BEACON_RPC_URL>
```

```admonish tip
All the parameters above can be provided as environment variables.
```

### Remote Endpoints
The mandatory arguments specify the endpoints that the validator should use to resolve disputes:
* `eth-rpc-url`: The parent chain (ethereum) endpoint for reading proposals.
* `beacon-rpc-url`: The DA layer (eth-beacon chain) endpoint for retrieving rollup data.
* `op-geth-url`: The rollup `op-geth` endpoint to read configuration data from.
* `op-node-url`: The rollup `op-node` endpoint to read sequencing proposals from.

### Cache Directory
The validator saves data to disk as it tracks on-chain proposals.
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

### Prover
The validator proving behavior can be customized through the following arguments:
* `kailua-cli`: The optional path of the external binary to call for custom proof generation.
* `num-concurrent-provers`: Number of provers to run simultaneously (Default: 1)
* `num-concurrent-preflights`: Number of threads per prover to use for fetching preflight data (Default: 4)
* `num-concurrent-proofs`: Number of threads per prover to use for computing sub-proofs (Default: 1)
* `segment-limit`: ZKVM Proving Segment Limit (Default 21)
* `max-witness-size`: Maximum input data byte size per sub-proof (Default 2684354560)

### Wallet
The validator requires a funded wallet to be able to publish fault proofs on chain, and an (optional) alternative address
to direct fault proof submission payouts towards.
This wallet can be specified directly as a private key or as an external AWS/GCP signer.
* `validator-key`: The private key for the validator wallet.
* `payout-recipient-address`: The ethereum address to use as the recipient of fault proof payouts.
* `validator-aws-key-id`: AWS KMS Key ID
* `validator-google-project-id`: GCP KMS Project ID
* `validator-google-location`: GCP KMS Location
* `validator-google-keyring`: GCP KMS Keyring Name
* `validator-google-key-name`: GCP KMS Key name

```admonish tip
`validator-key` can be replaced with the corresponding AWS/GCP parameters as described [here](upgrade.md#kms-support).
```

```admonish warning
You must keep your validator's wallet well funded to guarantee the liveness of your rollup and prevent faulty proposals
from delaying the finality of honest sequencing proposals.
```

```admonish success
Running `kailua-cli validate` should monitor your rollup for any disputes and generate the required proofs!
```

### Transactions
You can control transaction publication through the two following parameters:
* `txn-timeout`: A timeout in seconds for transaction broadcast (default 120)
* `exec-gas-premium`: An added premium percentage to estimated execution gas fees (Default 25)

The premium parameter increases the internally estimated fees by the specified percentage.

### Upgrades
If you re-deploy the KailuaTreasury/KailuaGame contracts to upgrade your fault proof system, you will need to restart
your validator (and proposer).
By default, the validator (and proposer) will use the latest contract deployment available upon start up, and ignore any
proposals not made using them.
If you wish to start a validator for a past deployment, you can explicitly specify the deployed KailuaGame contract
address using the optional `kailua-game-implementation` parameter.
```admonish note
The validator will not generate any proofs for proposals made using a different deployment than the one used at start up.
```

## Validity Proof Generation
Instead of only generating fault proofs, the validator can be instructed to generate a validity proof for every correct
canonical proposal it encounters to fast-forward finality until a specified block height.
This is configured using the below parameter:
*  `fast-forward-target`: The L2 block height until which validity proofs should be computed.

```admonish note
To indefinitely power a validity-proof only rollup, this value can be specified to the maximum 64-bit value of
`18446744073709551615`.
```

```admonish success
Running `kailua-cli validate` with the above parameter should generate a validity proof as soon as a correct proposal
is made by an honest proposer!
```

## Delegated Proof Generation
Extra parameters and environment variables can be specified to determine exactly where the RISC Zero proof
generation takes place.
Running using only the parameters above will generate proofs using the local RISC Zero prover available to the validator.
Alternatively, proof generation can be delegated to an external service such as [Bonsai](https://risczero.com/bonsai),
or to the decentralized [Boundless proving network](https://docs.beboundless.xyz/).

```admonish note
All data required to generate the proof can be publicly derived from the public chain data available for your rollup,
making this process safe to delegate.
```

### Bonsai
Enabling proving using [Bonsai](https://risczero.com/bonsai) requires you to set the following two environment variables before running the validator:
* `BONSAI_API_KEY`: Your Bonsai API key.
* `BONSAI_API_URL`: Your Bonsai API url.

```admonish success
Running `kailua-cli validate` with these two environment variables should now delegate all validator proving to [Bonsai](https://risczero.com/bonsai)!
```

### Boundless
When delegating generation of Kailua Fault proofs to the decentralized [Boundless proving network](https://docs.beboundless.xyz/),
for every fault proof, a proof request is submitted to the network, where it goes through the standard
[proof life-cycle](https://docs.beboundless.xyz/provers/proof-lifecycle) on boundless, before being published by
your validator to settle a dispute.

This functionality requires some additional parameters when starting the validator.
These parameters can be passed in as CLI arguments or set as environment variables

#### Proof Requests
The following first set of parameters determine where/how requests are made:
* `boundless-rpc-url`: The rpc endpoint of the L1 chain where the Boundless network is deployed.
* `boundless-wallet-key`: The wallet private key to use to send proof request transactions.
* `boundless-order-stream-url`: (Optional) The URL to use for off-chain order submission.
* `boundless-chain-id`: EIP-155 chain ID of the network hosting Boundless.
* `boundless-verifier-router-address`: Address of the RiscZeroVerifierRouter contract.
* `boundless-set-verifier-address`: The address of the RISC Zero verifier supporting aggregated proofs for order validation.
* `boundless-market-address`: The address of the Boundless market contract.
* `boundless-stake-token-address`: Address of the stake token contract. The staking token is an ERC-20.
* `boundless-lookback`: (Defaults to `5`) The number of previous proof requests to inspect for duplicates before making a new proof request.
* `boundless-cycle-min-wei`: (Defaults to `100000000`) Starting price (wei) per cycle of proving.
* `boundless-cycle-max-wei`: (Defaults to `200000000`) Maximum price (wei) per cycle of proving.
* `boundless-order-ramp-up-factor`: (Defaults to `0.25`) Multiplier for order price to ramp up to maximum.
* `boundless-order-lock-timeout-factor`: (Defaults to `3`) Multiplier for order fulfillment timeout after locking.
* `boundless-order-expiry-factor`: (Defaults to `10`) Multiplier for order expiry timeout after creation.
* `boundless-order-check-interval`: (Defaults to `12`) Time in seconds between attempts to check order status.

```admonish note
Order timeouts are set by default to the number of megacycles in a proof request.
The multipliers allow you to scale these timeouts according to your expected proving speeds.
The default scale values give a 1 MHz prover 3x the amount of time it needs to fulfill a request once it's locked, and 
10x its expected proving time as overall timeout.
```

#### Storage Provider
The below second set of parameters determine where the proven executable and its input are stored:
* `storage-provider`: One of `s3`, `pinata`, or `file`.
* `s3-access-key`: The `s3` access key.
* `s3-secret-key`: The `s3` secret key.
* `s3-bucket`: The `s3` bucket.
* `s3-url`: The `s3` url.
* `s3-use-presigned`: Use presigned URLs for S3.
* `aws-region`: The `s3` region.
* `pinata-jwt`: The private `pinata` jwt.
* `pinata-api-url`: The `pinata` api URL.
* `ipfs-gateway-url`: The `pinata` gateway URL.
* `file-path`: The file storage provider path.

```admonish success
Running `kailua-cli validate` with the above extra arguments should now delegate all validator proving to the [Boundless proving network](https://docs.beboundless.xyz/)!
```


## Advanced Settings

When manually computing individual proofs, the following parameters (or equiv. env. vars) take effect:
* `SKIP_AWAIT_PROOF`: Skips waiting for the proving process to complete on Bonsai/Boundless.
* `SKIP_DERIVATION_PROOF`: Skips provably deriving L2 transactions using L1 data.
