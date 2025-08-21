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

contract CostsTest is KailuaTest {
    KailuaTreasury treasury;
    KailuaGame game;
    KailuaTournament anchor;

    function setUp() public override {
        super.setUp();
    }

    function test_propose_n(uint256 n) internal {
        // Deploy dispute contracts
        (treasury, game, anchor) = deployKailua(
            uint64(4096 * n + 1), // n blobs
            uint64(0x01), // 1 block per commitment
            sha256(abi.encodePacked(bytes32(0x00))), // arbitrary genesis hash
            uint64(0x0), // genesis
            uint256(0), // start l2 from a while ago
            uint256(0x1), // 1-second block times
            uint64(0x0) // no dispute timeout
        );
        // Succeed in proposing with blob hash
        bytes32[] memory blobs = new bytes32[](n);
        for (uint256 i = 0; i < n; i++) {
            blobs[i] = this.versionedKZGHash(BLOB_NZ_COMMIT);
        }
        vm.blobhashes(blobs);
        vm.warp(
            game.GENESIS_TIME_STAMP() + game.PROPOSAL_OUTPUT_COUNT() * game.OUTPUT_BLOCK_SPAN() * game.L2_BLOCK_TIME()
        );
        treasury.propose(
            Claim.wrap(0x0001010000010100000010100000101000001010000010100000010100000101),
            abi.encodePacked(
                uint64(game.PROPOSAL_OUTPUT_COUNT() * game.OUTPUT_BLOCK_SPAN()), uint64(anchor.gameIndex()), uint64(0)
            )
        );
        console2.log(vm.lastCallGas().gasTotalUsed);
    }

    function test_propose_cost() public {
        for (uint256 i = 0; i <= 6; i++) {
            test_propose_n(i);
        }
    }
}
