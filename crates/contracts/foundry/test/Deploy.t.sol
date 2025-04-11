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

import {ScaffoldFactory} from "./ScaffoldFactory.sol";
import {ScaffoldPortal} from "./ScaffoldPortal.sol";
import {KailuaTreasury} from "../src/KailuaTreasury.sol";
import "../src/vendor/FlatOPImportV1.4.0.sol";
import "../src/vendor/FlatR0ImportV2.0.2.sol";
import {KailuaGame} from "../src/KailuaGame.sol";

contract DeployTest is Test {
    ScaffoldFactory factory;
    ScaffoldPortal portal;

    function setUp() public {
        factory = new ScaffoldFactory();
        portal = new ScaffoldPortal(factory);
    }

    function test_canDeployContracts() public {
        KailuaTreasury treasury = new KailuaTreasury(
            IRiscZeroVerifier(address(0x0)),
            bytes32(0x0),
            bytes32(0x0),
            uint256(0x1),
            uint256(0x1),
            GameType.wrap(1337),
            OptimismPortal2(payable(address(portal))),
            Claim.wrap(bytes32(0x0)),
            uint64(0x0)
        );
        console.logAddress(address(treasury));
        KailuaGame game = new KailuaGame(treasury, uint256(0x0), uint256(0x0), uint256(0x0), Duration.wrap(0x0));
        console.logAddress(address(game));
    }
}
