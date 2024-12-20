# Setup

Make sure to first install the [prerequisites](quickstart.md#prerequisites) from the quickstart
section before proceeding.

## Installation

Before you can start migrating your rollup, you'll need to build and install Kailua's binaries by calling the following
commands from the root project directory:

```admonish tip
Do not run these `install` commands in parallel.
Each binary installation will take time to reproducibly build the FPVM program in release mode.
If you install them in parallel, GitHub may throttle you, leading to a docker build error.
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

Once your installation is successful, you should be able to run the following command to fetch the Kailua configuration
parameters for your rollup instance:

```shell
kailua-cli config --op-node-url [YOUR_OP_NODE_URL] --op-geth-url [YOUR_OP_GETH_URL] --eth-rpc-url [YOUR_ETH_RPC_URL]
```

Running the above command against the respective op-sepolia endpoints should produce the following output:
```
RISC0_VERSION: 1.2.0
FPVM_IMAGE_ID: 0xA1FC2FD8A2EEE54047591648D90D7692D2E2EA9E5F1160CF7575AF2B03E16BBE
CONTROL_ROOT: 0x8CDAD9242664BE3112ABA377C5425A4DF735EB1C6966472B561D2855932C0469
CONTROL_ID: 0x04446E66D300EB7FB45C9726BB53C793DDA407A62E9601618BB43C5C14657AC0
SET_BUILDER_ID: 0x744CCA56CDE6933DEA72752C78B4A6CA894ED620E8AF6437AB05FAD53BCEC40A
RISC_ZERO_VERIFIER: 0x925D8331DDC0A1F0D96E68CF073DFE1D92B69187
GENESIS_TIMESTAMP: 1691802540
BLOCK_TIME: 2
ROLLUP_CONFIG_HASH: 0xF9CDE5599A197A7615D7207E55188D9D1709073A67A8F2D53EB9184400D4FBCD
DISPUTE_GAME_FACTORY: 0x05F9613ADB30026FFD634F38E5C4DFD30A197FA1
OPTIMISM_PORTAL: 0x16FC5058F25648194471939DF75CF27A2FDC48BC
KAILUA_GAME_TYPE: 1337
```

```admonish warning
Make sure that your `FPVM_IMAGE_ID` matches the value above.
This value determines the exact program used to prove faults.
```

```admonish note
If your `RISC_ZERO_VERIFIER` value is blank, this means that your rollup might be deployed on a base layer that does
not have a deployed RISC Zero zkVM verifier contract.
This means you might have to deploy your own verifier.
Always revise the RISC Zero [documentation](https://dev.risczero.com/api/blockchain-integration/contracts/verifier)
to double-check verifier availability.
```

Once you have these values you'll need to save them for later use during migration.