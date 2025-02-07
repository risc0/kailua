```sh

# Deploy 
forge script script/TestToken.s.sol:DeployToken --rpc-url http://127.0.0.1:9545 --private-key 0x8b3a350cf5c34c9194ca85829a2df0ec3153be0318b5e2d3348e872092edffba --broadcast

# Mint
forge script script/TestToken.s.sol:MintTokens --rpc-url http://127.0.0.1:9545 --private-key 0x8b3a350cf5c34c9194ca85829a2df0ec3153be0318b5e2d3348e872092edffba --broadcast

# Transfer
forge script script/TestToken.s.sol:TransferTokens --rpc-url http://127.0.0.1:9545 --private-key 0x8b3a350cf5c34c9194ca85829a2df0ec3153be0318b5e2d3348e872092edffba --broadcast

```

Note: private key is just a test key
