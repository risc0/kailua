# Kailua RPC

The Kailua RPC watches a Kailua chain deployment for proposals and keeps track of the canonical dispute game
contracts that can be safely used to initiate withdrawals on OP Stack knowing that they are guaranteed to eventually
be resolved.

## Methods

The `kailua` RPC namespace contains the following method:
* `kailua_gameAddressForBlockByNumber`: Returns the address of the earliest Kailua dispute game contract that can be 
  safely used to prove/finalize a withdrawal in the Optimism portal for any withdrawal initiated at the given L2 block
  number. (Returns null if no such contract yet exists.)

```admonish example
Using the local devnet deployment, the RPC can be queried as follows:

`cast rpc -r http://127.0.0.1:1337 kailua_gameAddressForBlockByNumber 200`
```

## Usage

Starting the Kailua RPC is straightforward:

```shell
kailua-cli rpc [OPTIONS] --op-node-url <OP_NODE_URL> --op-geth-url <OP_GETH_URL> --eth-rpc-url <ETH_RPC_URL> --beacon-rpc-url <BEACON_RPC_URL>
```

```admonish tip
All the parameters above can be provided as environment variables.
```

### Remote Endpoints
The mandatory arguments specify the endpoints that the RPC should use to track sequencing proposals:
* `eth-rpc-url`: The parent chain (Ethereum) endpoint for reading proposals.
* `beacon-rpc-url`: The DA layer (eth-beacon chain) endpoint for retrieving rollup data.
* `op-geth-url`: The rollup `op-geth` endpoint to read configuration data from.
* `op-node-url`: The rollup `op-node` endpoint to read sequencing proposals from.

### RPC Endpoint
These optional arguments configure the endpoint that the RPC server listens on:
* `socket-addr`: Socket for http or ws connections.
* `disable-http`: Disables listening for RPC requests over HTTP.
* `disable-ws`: Disables listening for RPC requests over WS.

### Cache Directory
The RPC saves data to disk as it tracks on-chain proposals.
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

```admonish success
Running `kailua-cli rpc` should now serve incoming RPC requests for the `kailua` namespace!
```
