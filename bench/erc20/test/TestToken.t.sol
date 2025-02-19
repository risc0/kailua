// SPDX-License-Identifier: UNLICENSED
pragma solidity ^0.8.13;

import {Test, console} from "forge-std/Test.sol";
import {TestToken} from "../src/TestToken.sol";

contract TestTokenTest is Test {
    TestToken public token;

    function setUp() public {
        token = new TestToken();
    }

    function testInitialSupply() public view {
        assertEq(token.totalSupply(), 1000000 * 10 ** 18);
        assertEq(token.balanceOf(address(this)), 1000000 * 10 ** 18);
    }

    function testMint() public {
        token.mint(address(1), 1000);
        assertEq(token.balanceOf(address(1)), 1000);
    }

    function testBurn() public {
        uint256 initialBalance = token.balanceOf(address(this));
        token.burn(1000);
        assertEq(token.balanceOf(address(this)), initialBalance - 1000);
    }
}
