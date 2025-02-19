## ERC20 Benchmarking

Start the local devnet.

Deploy and mint the test ERC20 Token to the test address. 

```sh
cd erc20

# Deploy 
forge script script/TestToken.s.sol:DeployToken --rpc-url http://127.0.0.1:9545 --private-key 0x8b3a350cf5c34c9194ca85829a2df0ec3153be0318b5e2d3348e872092edffba --broadcast

# Mint
forge script script/TestToken.s.sol:MintTokens --rpc-url http://127.0.0.1:9545 --private-key 0x8b3a350cf5c34c9194ca85829a2df0ec3153be0318b5e2d3348e872092edffba --broadcast
```

Create a list of generated Ethereum addresses, called `addresses.txt`. It should be a list like: 

```
0x91169a5d11Fe07dF452D62161bD0c8F424Bc0E13
0xcf161E9865A62cb308363DaE1EAaD09Ce24f6e9d
0x4d3183fE827C8334600E8B1FB1B8F53c807F31fD
...
```

The number of addresses will determine the number of tx's our publisher will create and publish.

# Benchmarking

There a three shell scripts to assist with benchmarking ERC20 txs. 

`block_monitor.sh` is for logging. 

`publisher.sh` is for creating and publishing transactions on the chain. 

`runner.sh` is for running the prover with different iterations. Will output a csv parsed from the log. 


Note: private key is just a test key
