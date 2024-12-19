# Setup

Make sure to first install the [prerequisites](quickstart.md#prerequisites) from the quickstart
section before proceeding.

## Installation

Before you can start migrating your rollup, you'll need to build and install Kailua's binaries by calling the following
commands from the root project directory:

```admonish tip
Do not run these commands in parallel.
Each of these commands will take time to build the FPVM program in release mode.
If you do, GitHub may throttle you, leading to a docker build error.
```

### CLI Binary
```shell
cargo install kailua-cli --path bin/cli
```

### Prover Binary
```admonish info
At the cost of longer compilation time, you can embed the RISC Zero prover logic into `kailua-host` instead of having 
it utilize your locally installed RISC Zero `r0vm`.
To do this, add `-F prove` to the command below.
```

```shell
cargo install kailua-host --path bin/host
```


## Configuration

Once your installation is successful, you should be able to run the following command to fetch the configuration
parameters of your rollup instance:

```shell
kailua-cli config --op-node-url [YOUR_OP_NODE_URL] --op-geth-url [YOUR_OP_GETH_URL] --eth-rpc-url [YOUR_ETH_RPC_URL]
```

Running the above command against the respective op-sepolia endpoints should produce the following output:
```
GENESIS_TIMESTAMP: 1734514840
BLOCK_TIME: 2
ROLLUP_CONFIG_HASH: 0xF41DDEEDA78780B0ACD5F8B66ED87E212E492E0ED567E6F4CFC8B16825311FD3
DISPUTE_GAME_FACTORY: 0xD34052D665891976EE71E097EAAF03DF51E9E3D5
OPTIMISM_PORTAL: 0x6509F2A854BA7441039FCE3B959D5BADD2FFCFCD
KAILUA_GAME_TYPE: 1337
FPVM_IMAGE_ID: 0xFD73222C8B0789FF0BEA5D40227D5F9186F1223A1DADEF64DA54F3C22094A4F3
RISC_ZERO_VERIFIER: 0x
SET_BUILDER_ID: 0x744CCA56CDE6933DEA72752C78B4A6CA894ED620E8AF6437AB05FAD53BCEC40A
CONTROL_ROOT: 0x8CDAD9242664BE3112ABA377C5425A4DF735EB1C6966472B561D2855932C0469
CONTROL_ID: 0x04446E66D300EB7FB45C9726BB53C793DDA407A62E9601618BB43C5C14657AC0
```

```admonish note
If your `RISC_ZERO_VERIFIER` value is blank, this means that your rollup might be deployed on a base layer that does
not have a deployed RISC Zero zkVM verifier contract.
This means you might have to deploy your own verifier.
Always revise the RISC Zero [documentation](https://dev.risczero.com/api/blockchain-integration/contracts/verifier)
to double-check verifier availability.
```

```admonish warning
Make sure that your `FPVM_IMAGE_ID` matches the value above.
This value determines the exact program used to prove faults.
```

Once you have these values you'll need to save them for later use during contract deployment.