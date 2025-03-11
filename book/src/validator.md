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
  --validator-key [YOUR_PROPOSER_WALLET_PRIVATE_KEY] \
  --payout-recipient-address [YOUR_FAULT_PROOF_PAYOUT_RECIPIENT]
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
The validator requires a funded wallet to be able to publish fault proofs on chain, and an (optional) alternative address
to direct fault proof submission payouts towards
* `validator-key`: The private key for the validator wallet.
* `payout-recipient-address`: The ethereum address to use as the recipient of fault proof payouts.

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
* `boundless-rpc-url`: The rpc endpoint of the L1 chain where the Boundless network is deployed.
* `boundless-wallet-key`: The wallet private key to use to send proof request transactions.
* `boundless-offchain`: (Optional) Flag instructing whether to submit proofs off-chain.
* `boundless-order-stream-url`: (Optional) The URL to use for off-chain order submission.
* `boundless-set-verifier-address`: The address of the RISC Zero verifier supporting aggregated proofs for order validation.
* `boundless-market-address`: The address of the Boundless market contract.
* `boundless-lookback`: (Defaults to `5`) The number of previous proof requests to inspect for duplicates before making a new proof request.

#### Storage Provider
The below second set of parameters determine where the proven executable and its input are stored:
* `storage-provider`: One of `s3`, `pinata`, or `file`.
* `s3-access-key`: The `s3` access key.
* `s3-secret-key`: The `s3` secret key.
* `s3-bucket`: The `s3` bucket.
* `s3-url`: The `s3` url.
* `aws-region`: The `s3` region.
* `pinata-jwt`: The private `pinata` jwt.
* `pinata-api-url`: The `pinata` api URL.
* `ipfs-gateway-url`: The `pinata` gateway URL.
* `file-path`: The file storage provider path.

```admonish success
Running `kailua-cli validate` with the above extra arguments should now delegate all validator proving to the [Boundless proving network](https://docs.beboundless.xyz/)!
```

## Advanced Settings

Fault/Validity proof generation can be fine-tuned via the two following environment variables:
* `NUM_CONCURRENT_HOSTS`: (default 1) The maximum number of kailua-host instances to run concurrently in the validator.
* `NUM_CONCURRENT_PREFLIGHTS`: (default 4) Sets the number of concurrent data preflights per proving task.
* `NUM_CONCURRENT_PROOFS`: (default 1) Sets the number of concurrent proofs to seek per proving task.
* `SEGMENT_LIMIT`: The [segment size limit](https://docs.rs/risc0-zkvm/1.2.3/risc0_zkvm/struct.ExecutorEnvBuilder.html#method.segment_limit_po2) used for local proving (Default 21).
* `MAX_WITNESS_SIZE`: The maximum input size per single proof (Default 100MB).

When manually computing individual proofs, the following parameters (or equiv. env. vars) take effect:
* `SKIP_DERIVATION_PROOF`: Skips provably deriving L2 transactions using L1 data.