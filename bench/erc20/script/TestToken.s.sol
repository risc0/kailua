// SPDX-License-Identifier: UNLICENSED
pragma solidity ^0.8.13;

import {Script, console} from "forge-std/Script.sol";
import {TestToken} from "../src/TestToken.sol";

contract BaseScript is Script {
    function readTokenAddress() internal view returns (address) {
        string memory addr = vm.readFile("./../erc20_address.txt");
        return vm.parseAddress(addr);
    }
}

contract DeployToken is Script {
    function run() public {
        vm.startBroadcast();
        TestToken counter = new TestToken();
        vm.stopBroadcast();
        vm.writeFile("./../erc20_address.txt", vm.toString(address(counter)));
    }
}

contract MintTokens is BaseScript {
    TestToken token;
    uint256 constant INITIAL_BALANCE = 100_000_000_000 * 10 ** 18;

    function run() public {
        token = TestToken(payable(readTokenAddress()));
        address sender = msg.sender;

        vm.startBroadcast();

        token.mint(sender, INITIAL_BALANCE);
        console.log("Minted:", INITIAL_BALANCE, "for:", sender);

        vm.stopBroadcast();
    }
}
