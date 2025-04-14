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

import {KailuaTreasury} from "../src/KailuaTreasury.sol";
import {KailuaGame} from "../src/KailuaGame.sol";

contract KailuaTest is Test {
    /// @dev Allows for the creation of clone proxies with immutable arguments.
    using LibClone for address;

    DisputeGameFactory factory;
    OptimismPortal2 portal;

    function setUp() public {
        factory = DisputeGameFactory(address(new DisputeGameFactory()).clone(abi.encodePacked(msg.sender)));
        portal = OptimismPortal2(
            payable(
                address(new OptimismPortal2(0, 0)).clone(
                    abi.encodePacked(address(factory), address(0x0), address(0x0), uint32(0x0))
                )
            )
        );
    }
}
