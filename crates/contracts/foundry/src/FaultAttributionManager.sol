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

    /// @notice The game type ID
    GameType internal immutable GAME_TYPE;

    /// @notice The bond required to act as a proposer.
    uint256 internal immutable PROPOSER_BOND;

    /// @notice The bond required to act as a challenger.
    uint256 internal immutable CHALLENGER_BOND;

    // ------------------------------
    // Mutable storage
    // ------------------------------

    /// @notice The proposals created by proposers
    ProposalData[] public proposals;
    /// @notice The bonds locked by proposers
    mapping(address => uint256) public proposerBonds;
    /// @notice The blacklist of proposers proven dishonest
    mapping(address => bool) public proposerBlacklisted;
    /// @notice The index of the last proposal made by each proposer
    mapping(address => uint64) public proposerPreviousIndex;

    /// @notice The challenges created by challengers
    ChallengeData[] public challenges;
    /// @notice The bonds locked by challengers
    mapping(address => uint256) public challengerBonds;
    /// @notice The blacklist of challengers proven dishonest
    mapping(address => bool) public challengersBlacklisted;
    /// @notice The index of the last challenge made by each challenge
    mapping(address => uint64) public challengerPreviousIndex;

    constructor() {
        proposals.push(
            ProposalData({
                proposerAddress: address(0x0),
                proposalContract: IDisputeGame(address(0x0)),
                previousProposalIndex: 0,
                challengeCount: 0
            })
        );
        challenges.push(
            ChallengeData({
                challengerAddress: address(0x0),
                proposalIndex: 0,
                outputOffset: 0,
                previousChallengeIndex: 0
            })
        );
    }

    // ------------------------------
    // IFaultAttributionManager implementation
    // ------------------------------
    // todo: reorg protection + sanity checks

    /// @notice Creates a new dispute game for a proposer using the DisputeGameFactory
    function propose(Claim claimedOutputRoot, bytes calldata extraData) external payable {
        // Only proposers not on the blacklist may make new proposals
        if (proposerBlacklisted[msg.sender]) {
            revert BadAuth();
        }
        // supplement proposer's collateral with transferred value
        if (msg.value > 0) {
            proposerBonds[msg.sender] += msg.value;
        }
        // ensure that the proposer has the collateral staked to make this move
        if (proposerBonds[msg.sender] != PROPOSER_BOND) {
            revert IncorrectBondAmount();
        }
        // Invoke the dispute game factory to create a new game instance
        IDisputeGame disputeGame = DISPUTE_GAME_FACTORY.create(GAME_TYPE, claimedOutputRoot, extraData);
        // Record the proposer's move
        proposals.push(
            ProposalData({
                proposerAddress: msg.sender,
                proposalContract: disputeGame,
                previousProposalIndex: proposerPreviousIndex[msg.sender],
                challengeCount: 0
            })
        );
        proposerPreviousIndex[msg.sender] = uint64(proposals.length - 1);
    }

    function challenge(uint64 proposalIndex, uint64 outputOffset, uint64 challengePriority) external payable {
        require(proposalIndex > 0);
        // Only challengers not on the blacklist may make new challenges
        if (challengersBlacklisted[msg.sender]) {
            revert BadAuth();
        }
        // supplement challenger's collateral with transferred value
        if (msg.value > 0) {
            challengerBonds[msg.sender] += msg.value;
        }
        // ensure that the challengers has the collateral staked to make this move
        if (challengerBonds[msg.sender] != CHALLENGER_BOND) {
            revert IncorrectBondAmount();
        }
        // Prevent unnecessary challenges through challengePriority
        if ((proposals[proposalIndex].challengeCount++) != challengePriority) {
            revert AlreadyChallenged();
        }
        // Record the challenger's move
        challenges.push(
            ChallengeData({
                challengerAddress: msg.sender,
                proposalIndex: proposalIndex,
                outputOffset: outputOffset,
                previousChallengeIndex: challengerPreviousIndex[msg.sender]
            })
        );
        challengerPreviousIndex[msg.sender] = uint64(challenges.length - 1);
        // todo: forward challenge to dispute game contract
    }

    function prove(
        uint64 proposalIndex,
        uint64 outputOffset,
        bytes calldata encodedSeal,
        bytes32 acceptedOutput,
        bytes32 proposedOutput,
        bytes32 computedOutput,
        bytes[] calldata kzgProofs
    ) external payable {
        require(proposalIndex > 0);
        // Only challengers not on the blacklist may submit proofs
        if (challengersBlacklisted[msg.sender]) {
            revert BadAuth();
        }
        // supplement challenger's collateral with transferred value
        if (msg.value > 0) {
            challengerBonds[msg.sender] += msg.value;
        }
        // ensure that the challengers has the collateral staked to make this move
        if (challengerBonds[msg.sender] != CHALLENGER_BOND) {
            revert IncorrectBondAmount();
        }

        // todo: forward proof to dispute contract to validate
        // todo: if challenge proven faulty, slash & inherit the challenger
    }

    function resolve(uint64 proposalIndex, uint64 outputOffset) external payable {
        require(proposalIndex > 0);
        // todo: take into consideration canonical challenge priority

        // todo: if proposal proven faulty, slash the proposer & terminate all following proposals

        // todo: a resolution in favor of the challenger cannot happen until all prior proposals
        // todo: to this one are resolved in favor of the proposer, o/w evidence of challenger fault
    }
}
