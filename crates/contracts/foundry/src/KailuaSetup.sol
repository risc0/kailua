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
pragma solidity ^0.8.15;

import "./vendor/FlatOPImportV1.4.0.sol";
import "./vendor/FlatR0ImportV1.0.0.sol";
import "./KailuaLib.sol";
import "./KailuaTournament.sol";

contract KailuaSetup is KailuaTournament {
    /// @notice Semantic version.
    /// @custom:semver 0.1.0
    string public constant version = "0.1.0";

    // ------------------------------
    // Immutable configuration
    // ------------------------------

    /// @notice The anchored game type ID to clone
    GameType internal immutable ANCHORED_GAME_TYPE;

    /// @notice Returns the anchored game type.
    function anchoredGameType() external view returns (GameType anchoredGameType_) {
        anchoredGameType_ = ANCHORED_GAME_TYPE;
    }

    constructor(
        IRiscZeroVerifier _verifierContract,
        bytes32 _imageId,
        bytes32 _configHash,
        uint256 _proposalBlockCount,
        GameType _anchoredGameType,
        GameType _gameType,
        IAnchorStateRegistry _anchorStateRegistry
    )
        KailuaTournament(_verifierContract, _imageId, _configHash, _proposalBlockCount, _gameType, _anchorStateRegistry)
    {
        GAME_TYPE = _gameType;
        ANCHORED_GAME_TYPE = _anchoredGameType;
        ANCHOR_STATE_REGISTRY = _anchorStateRegistry;
    }

    // ------------------------------
    // IInitializable implementation
    // ------------------------------

    /// @inheritdoc IInitializable
    function initialize() external payable {
        // INVARIANT: The game must not have already been initialized.
        if (createdAt.raw() > 0) revert AlreadyInitialized();

        if ((rootClaim().raw() != bytes32(0)) || (l2BlockNumber() != 0)) {
            // Validate the cloned anchor state
            IDisputeGameFactory disputeGameFactory = ANCHOR_STATE_REGISTRY.disputeGameFactory();

            (IDisputeGame proxyAddress,) = disputeGameFactory.games(ANCHORED_GAME_TYPE, rootClaim(), this.extraData());

            IFaultDisputeGame anchoredGame = IFaultDisputeGame(address(proxyAddress));

            // Validate that the game is resolved correctly
            if (anchoredGame.status() != GameStatus.DEFENDER_WINS) revert InvalidAnchoredGame();

            // Revert if different proposal root
            if (anchoredGame.rootClaim().raw() != rootClaim().raw()) revert UnexpectedRootClaim(rootClaim());

            // Revert if different proposal root block number
            if (anchoredGame.l2BlockNumber() != l2BlockNumber()) {
                revert BlockNumberMismatch(anchoredGame.l2BlockNumber(), l2BlockNumber());
            }
        }

        // Set the game's starting timestamp
        createdAt = Timestamp.wrap(uint64(block.timestamp));
    }

    // ------------------------------
    // IDisputeGame implementation
    // ------------------------------

    /// @inheritdoc IDisputeGame
    function resolve() external returns (GameStatus status_) {
        // INVARIANT: Resolution cannot occur unless the game is currently in progress.
        if (status != GameStatus.IN_PROGRESS) revert GameNotInProgress();

        // INVARIANT: Only the factory owner can resolve the setup game
        //        IDisputeGameFactory disputeGameFactory = ANCHOR_STATE_REGISTRY.disputeGameFactory();
        //        if (msg.sender != disputeGameFactory.owner())

        // Update the status and emit the resolved event, note that we're performing a storage update here.
        emit Resolved(status = status_ = GameStatus.DEFENDER_WINS);

        // Mark resolution timestamp
        resolvedAt = Timestamp.wrap(uint64(block.timestamp));

        // Try to update the anchor state, this should not revert.
        ANCHOR_STATE_REGISTRY.tryUpdateAnchorState();
    }

    // ------------------------------
    // Fault proving
    // ------------------------------

    /// @inheritdoc KailuaTournament
    function verifyIntermediateOutput(
        uint32 outputNumber,
        bytes32 outputHash,
        bytes calldata blobCommitment,
        bytes calldata kzgProof
    ) external override returns (bool success) {
        success = false;
    }

    /// @inheritdoc KailuaTournament
    function getChallengerDuration() public view override returns (Duration duration_) {
        duration_ = Duration.wrap(0);
    }
}
