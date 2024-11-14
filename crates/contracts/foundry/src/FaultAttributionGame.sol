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

contract FaultAttributionGame is Clone, IFaultAttributionGame {
    /// @notice Semantic version.
    /// @custom:semver 0.1.0
    string public constant version = "0.1.0";

    // ------------------------------
    // Immutable template configuration
    // ------------------------------

    /// @notice The fault attribution manager
    IFaultAttributionManager internal immutable FAULT_ATTRIBUTION_MANAGER;

    /// @notice The maximum number of blocks a claim may cover
    uint256 internal immutable MAX_BLOCK_COUNT;

    /// @notice The duration after which the proposal is accepted
    Duration internal immutable MAX_CLOCK_DURATION;

    /// @notice The game type ID
    GameType internal immutable GAME_TYPE;

    /// @notice The anchor state registry.
    IAnchorStateRegistry internal immutable ANCHOR_STATE_REGISTRY;

    /// @notice Returns the maximum number of blocks that can be covered by this game
    function maxBlockCount() external view returns (uint256 maxBlockCount_) {
        maxBlockCount_ = MAX_BLOCK_COUNT;
    }

    /// @notice Returns the max clock duration.
    function maxClockDuration() external view returns (Duration maxClockDuration_) {
        maxClockDuration_ = MAX_CLOCK_DURATION;
    }

    /// @notice Returns the anchor state registry contract.
    function anchorStateRegistry() external view returns (IAnchorStateRegistry registry_) {
        registry_ = ANCHOR_STATE_REGISTRY;
    }

    constructor(
        IFaultAttributionManager _faultAttributionManager,
        uint256 _maxBlockCount,
        Duration _maxClockDuration,
        GameType _gameType,
        IAnchorStateRegistry _anchorStateRegistry
    ) {
        FAULT_ATTRIBUTION_MANAGER = _faultAttributionManager;
        GAME_TYPE = _gameType;
        MAX_BLOCK_COUNT = _maxBlockCount;
        MAX_CLOCK_DURATION = _maxClockDuration;
        ANCHOR_STATE_REGISTRY = _anchorStateRegistry;
    }

    // ------------------------------
    // IInitializable implementation
    // ------------------------------

    /// @notice Initializes the contract
    /// @dev This function may only be called once.
    function initialize() external payable {
        // INVARIANT: The game must not have already been initialized.
        if (createdAt.raw() > 0) revert AlreadyInitialized();

        // Revert if the calldata size is not the expected length.
        //
        // This is to prevent adding extra or omitting bytes from to `extraData` that result in a different game UUID
        // in the factory, but are not used by the game, which would allow for multiple dispute games for the same
        // output proposal to be created.
        //
        // Expected length: 0x72
        // - 0x04 selector
        // - 0x14 creator address
        // - 0x20 root claim
        // - 0x20 l1 head
        // - 0x18 extraData (0x08 l2BlockNumber, 0x08 parentGameIndex, 0x08 proposalBlobHashCount)
        // - 0x02 CWIA bytes
        if (msg.data.length != 0x72) {
            revert BadExtraData();
        }

        // Do not allow the game to be initialized if the root claim corresponds to a block at or before the
        // starting block number. (0xf40239db)
        if (l2BlockNumber() <= startingBlockNumber()) revert UnexpectedRootClaim(rootClaim());

        // Do not initialize a game that covers more blocks than permitted
        if (l2BlockNumber() - startingBlockNumber() > MAX_BLOCK_COUNT) {
            revert BlockCountExceeded(l2BlockNumber(), startingBlockNumber());
        }

        // Store the intermediate output blob hashes
        uint64 hashes = proposalBlobHashCount();
        for (uint64 i = 0; i < hashes; i++) {
            bytes32 hash = blobhash(i);
            if (hash == 0x0) {
                revert BlobHashMissing(i, hashes);
            }
            proposalBlobHashes.push(Hash.wrap(blobhash(i)));
        }

        // Ensure new proposals only come through the manager
        if (gameCreator() != address(FAULT_ATTRIBUTION_MANAGER)) {
            revert BadAuth();
        }

        // Set the game's starting timestamp
        createdAt = Timestamp.wrap(uint64(block.timestamp));
    }

    // ------------------------------
    // IDisputeGame implementation
    // ------------------------------

    /// @notice The starting timestamp of the game
    Timestamp public createdAt;

    /// @inheritdoc IDisputeGame
    Timestamp public resolvedAt;

    /// @inheritdoc IDisputeGame
    GameStatus public status;

    /// @notice Returns the game type.
    function gameType() external view returns (GameType gameType_) {
        gameType_ = GAME_TYPE;
    }

    /// @inheritdoc IDisputeGame
    function gameCreator() public pure returns (address creator_) {
        creator_ = _getArgAddress(0x00);
    }

    /// @inheritdoc IDisputeGame
    function rootClaim() public pure returns (Claim rootClaim_) {
        rootClaim_ = Claim.wrap(_getArgBytes32(0x14));
    }

    /// @inheritdoc IDisputeGame
    function l1Head() public pure returns (Hash l1Head_) {
        l1Head_ = Hash.wrap(_getArgBytes32(0x34));
    }

    /// @notice Getter for the extra data.
    /// @dev `clones-with-immutable-args` argument #2
    /// @return extraData_ Any extra data supplied to the dispute game contract by the creator.
    function extraData() external pure returns (bytes memory extraData_) {
        // The extra data starts at the second word within the cwia calldata and
        // is 48 bytes long.
        extraData_ = _getArgBytes(0x54, 0x30);
    }

    /// @inheritdoc IDisputeGame
    function resolve() external returns (GameStatus status_) {
        // Ensure resolutions are only authorized by the manager
        if (msg.sender != address(FAULT_ATTRIBUTION_MANAGER)) {
            revert BadAuth();
        }

        // Try to update the anchor state, this should not revert.
        if (status_ == GameStatus.DEFENDER_WINS) {
            ANCHOR_STATE_REGISTRY.tryUpdateAnchorState();
        }
        // todo
    }

    /// @notice A compliant implementation of this interface should return the components of the
    ///         game UUID's preimage provided in the cwia payload. The preimage of the UUID is
    ///         constructed as `keccak256(gameType . rootClaim . extraData)` where `.` denotes
    ///         concatenation.
    /// @return gameType_ The type of proof system being used.
    /// @return rootClaim_ The root claim of the DisputeGame.
    /// @return extraData_ Any extra data supplied to the dispute game contract by the creator.
    function gameData() external view returns (GameType gameType_, Claim rootClaim_, bytes memory extraData_) {
        gameType_ = this.gameType();
        rootClaim_ = this.rootClaim();
        extraData_ = this.extraData();
    }

    // ------------------------------
    // Immutable instance data
    // ------------------------------

    Hash[] public proposalBlobHashes;

    uint64 public gameIndex;

    /// @notice The l2BlockNumber of the claim's output root.
    function l2BlockNumber() public pure returns (uint256 l2BlockNumber_) {
        l2BlockNumber_ = _getArgUint64(0x54);
    }

    /// @notice The index of the parent game in the `DisputeGameFactory`.
    function parentGameIndex() public pure returns (uint64 parentGameIndex_) {
        parentGameIndex_ = _getArgUint64(0x5C);
    }

    /// @notice The number of blobs submitted along with the proposal
    function proposalBlobHashCount() public pure returns (uint64 proposalBlobHashCount_) {
        proposalBlobHashCount_ = _getArgUint64(0x64);
    }

    /// @notice The parent game contract.
    function parentGame() public view returns (FaultAttributionGame parentGame_) {
        (GameType parentGameType,, IDisputeGame parentDisputeGame) =
            ANCHOR_STATE_REGISTRY.disputeGameFactory().gameAtIndex(parentGameIndex());

        // Only allow fault claim games to be based off of other instances of the same game type
        if (parentGameType.raw() != GAME_TYPE.raw()) revert GameTypeMismatch(parentGameType, GAME_TYPE);

        // Interpret parent game as another instance of this game type
        parentGame_ = FaultAttributionGame(address(parentDisputeGame));
    }

    /// @notice Only the starting block number of the game.
    function startingBlockNumber() public view returns (uint256 startingBlockNumber_) {
        startingBlockNumber_ = parentGame().l2BlockNumber();
    }

    /// @notice Only the starting output root of the game.
    function startingRootHash() public view returns (Hash startingRootHash_) {
        startingRootHash_ = Hash.wrap(parentGame().rootClaim().raw());
    }

    // ------------------------------
    // Utility methods
    // ------------------------------

    /// @notice Returns the amount of time elapsed on the potential challenger to the claim's chess clock. Maxes
    ///         out at `MAX_CLOCK_DURATION`.
    /// @return duration_ The time elapsed on the potential challenger to `_claimIndex`'s chess clock.
    function getChallengerDuration() public view returns (Duration duration_) {
        // INVARIANT: The game must be in progress to query the remaining time to respond to a given claim.
        if (status != GameStatus.IN_PROGRESS) {
            revert GameNotInProgress();
        }

        // Compute the duration elapsed of the potential challenger's clock.
        uint64 challengeDuration = uint64(block.timestamp - createdAt.raw());
        duration_ = challengeDuration > MAX_CLOCK_DURATION.raw() ? MAX_CLOCK_DURATION : Duration.wrap(challengeDuration);
    }
}
