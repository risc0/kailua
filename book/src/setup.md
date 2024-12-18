# Setup

Make sure to first install the [prerequisites](http://localhost:3000/quickstart.html#prerequisites) from the quickstart
section before proceeding.

## Installation

Before you can start migrating your rollup, you'll need to build and install Kailua's binaries by calling the following
commands from the root project directory:

```shell
cargo install kailua-cli --path bin/cli
```
```shell
cargo install kailua-host --path bin/host
```

```admonish tip
Do not run these commands in parallel.
Each of these commands will take time to build the FPVM program in release mode.
If you do, GitHub may throttle you, leading to a docker build error.
```

## Configuration

Once your installation is successful, you should be able to run the following command to fetch the configuration
parameters of your rollup instance:

```shell
kailua-cli config --op-node-url [YOUR_OP_NODE_URL] --op-geth-url [YOUR_OP_GETH_URL]
```

Running the above command against op-sepolia endpoints should produce the following output:
```
ROLLUP_CONFIG_HASH: 0xF9CDE5599A197A7615D7207E55188D9D1709073A67A8F2D53EB9184400D4FBCD
FPVM_IMAGE_ID: 0x58D1C127AEA2C9F208B6015E3FD6615E6815E6CBD5AADED4E619376AFADA70A4
RISC_ZERO_VERIFIER: 0x925D8331DDC0A1F0D96E68CF073DFE1D92B69187
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