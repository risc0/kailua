// Copyright 2025 RISC Zero, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.
//
// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.24;

import "./KailuaTest.t.sol";

contract DeployTest is KailuaTest {
    function setUp() public override {
        super.setUp();
    }

    function test_canDeployKailua() public {
        (KailuaTreasury treasury,, KailuaTournament anchor) = deployKailua(
            uint256(0x1),
            uint256(0x1),
            sha256(abi.encodePacked(bytes32(0x00))),
            uint64(0x0),
            uint256(0x0),
            uint256(0x0),
            uint256(0x0),
            uint64(0x0)
        );
        // Check anchor data
        vm.assertEq(treasury.lastResolved(), address(anchor));
        vm.assertEq(factory.gameCount() - 1, anchor.gameIndex());
        vm.assertEq(address(anchor.parentGame()), address(anchor));
        vm.assertEq(anchor.minCreationTime().raw(), anchor.createdAt().raw());
        vm.assertEq(anchor.getChallengerDuration(anchor.createdAt().raw()).raw(), 0);
        vm.assertEq(anchor.extraData(), abi.encodePacked(uint64(anchor.l2BlockNumber()), address(treasury)));
        vm.assertFalse(anchor.verifyIntermediateOutput(0, 0, hex"", hex""));
        KailuaTreasury anchorTreasury = KailuaTreasury(address(anchor));
        vm.assertEq(anchorTreasury.treasuryAddress(), address(treasury));
    }
}
