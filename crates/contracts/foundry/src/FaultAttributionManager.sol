// Copyright 2024 RISC Zero, Inc.
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

import "./vendor/FlatOPImportV1.4.0.sol";
import "./vendor/FlatR0ImportV1.0.0.sol";
import "./FaultLib.sol";

contract FaultAttributionManager is IFaultAttributionManager {
    /// @notice Semantic version.
    /// @custom:semver 0.1.0
    string public constant version = "0.1.0";

    // ------------------------------
    // Immutable configuration
    // ------------------------------

    /// @notice The dispute game factory contract.
    IDisputeGameFactory internal immutable DISPUTE_GAME_FACTORY;

    /// @notice The bond required to act as a proposer.
    uint256 internal immutable PROPOSER_BOND;

    // ------------------------------
    // Mutable storage
    // ------------------------------

    /// @notice The bonds locked by proposers
    mapping(address => uint256) public proposerBonds;

    // ------------------------------
    // IFaultAttributionManager implementation
    // ------------------------------

    /// @notice Validates that a new proposal may be submitted by a game creator
    /// @dev This function is safe to call from arbitrary sources.
    function propose() external payable {
        // Retrieve the proposer's address from the calling contract
        IFaultAttributionGame gameContract = IFaultAttributionGame(msg.sender);
        address proposer = gameContract.gameCreator();
        // supplement proposer's collateral with transferred value
        if (msg.value > 0) {
            proposerBonds[proposer] += msg.value;
        }
        // ensure that the proposer has enough collateral staked to make this move
        if (proposerBonds[proposer] < PROPOSER_BOND) {
            revert IncorrectBondAmount();
        }
    }
}
