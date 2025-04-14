// Copyright 2024, 2025 RISC Zero, Inc.
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

import {Test} from "forge-std/Test.sol";
import {console} from "forge-std/console.sol";

import "../src/vendor/FlatOPImportV1.4.0.sol";
import "../src/vendor/FlatR0ImportV2.0.2.sol";

import "../src/KailuaTournament.sol";
import "../src/KailuaTreasury.sol";
import "../src/KailuaGame.sol";

contract KailuaTest is Test {
    /// @dev Allows for the creation of clone proxies with immutable arguments.
    using LibClone for address;

    DisputeGameFactory factory;
    OptimismPortal2 portal;
    RiscZeroMockVerifier verifier;

    function setUp() public virtual {
        // OP Stack
        factory = DisputeGameFactory(address(new DisputeGameFactory()).clone());
        factory.initialize(address(this));
        portal = OptimismPortal2(payable(address(new OptimismPortal2(0, 0)).clone()));
        portal.initialize(
            factory, SystemConfig(address(0x0)), SuperchainConfig(address(0x0)), GameType.wrap(uint32(1337))
        );
        vm.assertEq(address(portal.disputeGameFactory()), address(factory));
        // RISC Zero
        verifier = new RiscZeroMockVerifier(bytes4(bytes32(uint256(0xFF))));
    }

    function deployKailua(
        uint256 proposalOutputCount,
        uint256 outputBlockSpan,
        bytes32 rootClaim,
        uint64 l2BlockNumber,
        uint256 genesisTimestamp,
        uint256 l2BlockTime,
        uint256 proposalTimeGap,
        uint64 maxClockDuration
    ) public returns (KailuaTreasury treasury, KailuaGame game) {
        // Kailua
        treasury = new KailuaTreasury(
            verifier,
            bytes32(0x0),
            bytes32(0x0),
            proposalOutputCount,
            outputBlockSpan,
            GameType.wrap(1337),
            OptimismPortal2(payable(address(portal))),
            Claim.wrap(rootClaim),
            l2BlockNumber
        );
        game = new KailuaGame(treasury, genesisTimestamp, l2BlockTime, proposalTimeGap, Duration.wrap(maxClockDuration));
        // Anchoring
        factory.setImplementation(GameType.wrap(1337), treasury);
        KailuaTournament anchor =
            treasury.propose(Claim.wrap(rootClaim), abi.encodePacked(l2BlockNumber, address(treasury)));
        anchor.resolve();
        // Proposals
        factory.setImplementation(GameType.wrap(1337), game);
    }
}
