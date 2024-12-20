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
kailua-cli validate \
  --eth-rpc-url [YOUR_ETH_RPC_URL] \
  --beacon-rpc-url [YOUR_BEACON_RPC_URL] \
  --op-geth-url [YOUR_OP_GETH_URL] \
  --op-node-url [YOUR_OP_NODE_URL] \
  --kailua-host [YOUR_KAILUA_HOST_BINARY_PATH] \
  --validator-key [YOUR_PROPOSER_WALLET_PRIVATE_KEY]
```

```admonish tip
All the parameters in this section can be provided as environment variables.
```

### Endpoints
The first four arguments specify the endpoints that the validator should use to generate fault proofs:
* `eth-rpc-url`: The parent chain (ethereum) endpoint for reading proposals and publishing proofs.
* `beacon-rpc-url`: The DA layer (eth-beacon chain) endpoint for retrieving rollup data.
* `op-geth-url`: The (archive) rollup `op-geth` endpoint to read fault proving witness data from.
* `op-node-url`: The rollup `op-node` endpoint to read sequencing proposals from.

### Prover
To create a fault proof, the validator invokes the `kailua-host` binary.
* `kailua-host`: The path to the `kailua-host` binary to call for proof generation.

### Wallet
The validator requires a funded wallet to be able to publish fault proofs on chain.
* `validator-key`: The private key for the validator wallet.

```admonish warning
You must keep your validator's wallet well funded to guarantee the liveness of your rollup and prevent faulty proposals
from delaying the finality of honest sequencing proposals.
```

```admonish success
Running `kailua-cli validate` should monitor your rollup for disputes and generate the required proofs!
```

## Delegated Proof Generation
Several extra parameters and environment variables can be specified to determine exactly where the RISC Zero proof
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
[proof life-cycle](https://docs.beboundless.xyz/introduction/proof-lifecycle) on boundless, before being published by
your validator to settle a dispute.

This functionality requires some additional parameters when starting the validator.
These parameters can be passed in as CLI arguments or set as environment variables

#### Proof Requests
The following first set of parameters determine where/how requests are made:
* `boundless_rpc_url`: The rpc endpoint of the L1 chain where the Boundless network is deployed.
* `boundless_wallet_key`: The wallet private key to use to send proof request transactions.
* `boundless_offchain`: (Optional) Flag instructing whether to submit proofs off-chain.
* `boundless_order_stream_url`: (Optional) The URL to use for off-chain order submission.
* `boundless_set_verifier_address`: The address of the RISC Zero verifier supporting aggregated proofs for order validation.
* `boundless_market_address`: The address of the Boundless market contract.
* `boundless_lookback`: (Defaults to `5`) The number of previous proof requests to inspect for duplicates before making a new proof request.

#### Storage Provider
The below second set of parameters determine where the proven executable and its input are stored:
* `storage_provider`: One of `s3`, `pinata`, or `file`.
* `s3_access_key`: The `s3` access key.
* `s3_secret_key`: The `s3` secret key.
* `s3_bucket`: The `s3` bucket.
* `s3_url`: The `s3` url.
* `aws_region`: The `s3` region.
* `pinata_jwt`: The private `pinata` jwt.
* `pinata_api_url`: The `pinata` api URL.
* `ipfs_gateway_url`: The `pinata` gateway URL.
* `file_path`: The file storage provider path.

```admonish success
Running `kailua-cli validate` with the above extra arguments should now delegate all validator proving to the [Boundless proving network](https://docs.beboundless.xyz/)!
```
