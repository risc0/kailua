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
import "./vendor/FlatR0ImportV1.2.0.sol";
import "./KailuaLib.sol";
import "./KailuaTournament.sol";
import "./KailuaTreasury.sol";

contract KailuaGame is KailuaTournament {
    /// @notice Semantic version.
    /// @custom:semver 0.1.0
    string public constant version = "0.1.0";

    // ------------------------------
    // Immutable configuration
    // ------------------------------

    /// @notice The duration after which the proposal is accepted
    Duration internal immutable MAX_CLOCK_DURATION;

    /// @notice The timestamp of the genesis l2 block
    uint256 internal immutable GENESIS_TIME_STAMP;

    /// @notice The time between l2 blocks
    uint256 internal immutable L2_BLOCK_TIME;

    /// @notice The minimum gap between the l1 and proposed l2 tip timestamps
    uint256 internal immutable PROPOSAL_TIME_GAP;

    /// @notice Returns the max clock duration.
    function maxClockDuration() public view returns (Duration maxClockDuration_) {
        maxClockDuration_ = MAX_CLOCK_DURATION;
    }

    /// @notice Returns the timestamp of the genesis L2 block
    function genesisTimeStamp() public view returns (uint256 genesisTimeStamp_) {
        genesisTimeStamp_ = GENESIS_TIME_STAMP;
    }

    /// @notice Returns the inter-block time of the L2
    function l2BlockTime() public view returns (uint256 l2BlockTime_) {
        l2BlockTime_ = L2_BLOCK_TIME;
    }

    /// @notice Returns the required gap between the current l1 timestamp and the proposal's l2 timestamp
    function proposalTimeGap() public view returns (uint256 proposalTimeGap_) {
        proposalTimeGap_ = PROPOSAL_TIME_GAP;
    }

    constructor(
        IKailuaTreasury _kailuaTreasury,
        IRiscZeroVerifier _verifierContract,
        bytes32 _imageId,
        bytes32 _configHash,
        uint256 _proposalBlockCount,
        GameType _gameType,
        IDisputeGameFactory _disputeGameFactory,
        uint256 _genesisTimeStamp,
        uint256 _l2BlockTime,
        uint256 _proposalTimeGap,
        Duration _maxClockDuration
    )
        KailuaTournament(
            _kailuaTreasury,
            _verifierContract,
            _imageId,
            _configHash,
            _proposalBlockCount,
            _gameType,
            _disputeGameFactory
        )
    {
        MAX_CLOCK_DURATION = _maxClockDuration;
        GENESIS_TIME_STAMP = _genesisTimeStamp;
        L2_BLOCK_TIME = _l2BlockTime;
        PROPOSAL_TIME_GAP = _proposalTimeGap;
    }

    // ------------------------------
    // IInitializable implementation
    // ------------------------------

    /// @inheritdoc IInitializable
    function initialize() external payable override {
        super.initializeInternal();

        // Revert if the calldata size is not the expected length.
        //
        // This is to prevent adding extra or omitting bytes from to `extraData` that result in a different game UUID
        // in the factory, but are not used by the game, which would allow for multiple dispute games for the same
        // output proposal to be created.
        //
        // Expected length: 0x72
        // - 0x04 selector                      0x00 0x04
        // - 0x14 creator address               0x04 0x18
        // - 0x20 root claim                    0x18 0x38
        // - 0x20 l1 head                       0x38 0x58
        // - 0x18 extraData:                    0x58 0x70
        //      + 0x08 l2BlockNumber            0x58 0x60
        //      + 0x08 parentGameIndex          0x60 0x68
        //      + 0x08 duplicationCounter)      0x68 0x70
        // - 0x02 CWIA bytes                    0x70 0x72
        if (msg.data.length != 0x72) {
            revert BadExtraData();
        }

        // Do only allow monotonic duplication counter
        uint256 duplicationCounter_ = duplicationCounter();
        if (duplicationCounter_ > 0) {
            bytes memory extra = abi.encodePacked(msg.data[0x58:0x68], uint64(duplicationCounter_ - 1));
            (IDisputeGame previousDuplicate,) = DISPUTE_GAME_FACTORY.games(GAME_TYPE, rootClaim(), extra);
            if (address(previousDuplicate) == address(0x0)) {
                revert InvalidDuplicationCounter();
            }
        }

        // Do not allow the game to be initialized if the root claim corresponds to a block at or before the
        // starting block number. (0xf40239db)
        uint256 thisL2BlockNumber = l2BlockNumber();
        uint256 prevL2BlockNumber = parentGame().l2BlockNumber();
        if (thisL2BlockNumber <= prevL2BlockNumber) {
            revert UnexpectedRootClaim(rootClaim());
        }

        // Do not initialize a game that does not cover the required number of l2 blocks
        if (thisL2BlockNumber - prevL2BlockNumber != PROPOSAL_BLOCK_COUNT) {
            revert BlockCountExceeded(thisL2BlockNumber, prevL2BlockNumber);
        }

        // Store the intermediate output blob hashes
        for (uint256 i = 0; i < PROPOSAL_BLOBS; i++) {
            bytes32 hash = blobhash(i);
            if (hash == 0x0) {
                revert BlobHashMissing(i, PROPOSAL_BLOBS);
            }
            proposalBlobHashes.push(Hash.wrap(hash));
        }

        // Allow only the treasury to create new games
        if (gameCreator() != address(KAILUA_TREASURY)) {
            revert Blacklisted(gameCreator(), address(KAILUA_TREASURY));
        }

        // Register this new game in the parent game's contract
        parentGame().appendChild();

        // Do not permit proposals of l2 block is still inside the gap
        if (block.timestamp <= GENESIS_TIME_STAMP + thisL2BlockNumber * L2_BLOCK_TIME + PROPOSAL_TIME_GAP) {
            revert ClockTimeExceeded();
        }
    }

    // ------------------------------
    // IDisputeGame implementation
    // ------------------------------

    /// @inheritdoc IDisputeGame
    function extraData() external pure returns (bytes memory extraData_) {
        // The extra data starts at the second word within the cwia calldata and
        // is 24 bytes long.
        extraData_ = _getArgBytes(0x54, 0x18);
    }

    /// @inheritdoc IDisputeGame
    function resolve() external returns (GameStatus status_) {
        // INVARIANT: Resolution cannot occur unless the game is currently in progress.
        if (status != GameStatus.IN_PROGRESS) {
            revert GameNotInProgress();
        }

        // INVARIANT: Optimistic resolution cannot occur unless parent game is resolved.
        KailuaTournament parentGame_ = parentGame();
        if (parentGame_.status() != GameStatus.DEFENDER_WINS) {
            revert OutOfOrderResolution();
        }

        // INVARIANT: Cannot resolve unless the clock has expired
        if (getChallengerDuration(block.timestamp).raw() > 0) {
            revert ClockNotExpired();
        }

        // INVARIANT: Can only resolve the last remaining child
        if (parentGame_.pruneChildren() != this) {
            revert ProvenFaulty();
        }

        // Mark resolution timestamp
        resolvedAt = Timestamp.wrap(uint64(block.timestamp));

        // Update the status and emit the resolved event, note that we're performing a storage update here.
        emit Resolved(status = status_ = GameStatus.DEFENDER_WINS);
    }

    // ------------------------------
    // Immutable instance data
    // ------------------------------

    /// @notice The index of the parent game in the `DisputeGameFactory`.
    function parentGameIndex() public pure returns (uint64 parentGameIndex_) {
        parentGameIndex_ = _getArgUint64(0x5C);
    }

    /// @notice The number of duplicate proposals preceeding this one.
    function duplicationCounter() public pure returns (uint64 duplicationCounter_) {
        duplicationCounter_ = _getArgUint64(0x64);
    }

    /// @inheritdoc KailuaTournament
    function parentGame() public view override returns (KailuaTournament parentGame_) {
        (GameType parentGameType,, IDisputeGame parentDisputeGame) = DISPUTE_GAME_FACTORY.gameAtIndex(parentGameIndex());

        // Only allow fault claim games to be based off of other instances of the same game type
        if (parentGameType.raw() != GAME_TYPE.raw()) revert GameTypeMismatch(parentGameType, GAME_TYPE);

        // Interpret parent game as another instance of this game type
        parentGame_ = KailuaTournament(address(parentDisputeGame));
    }

    // ------------------------------
    // Fault proving
    // ------------------------------

    /// @inheritdoc KailuaTournament
    function verifyIntermediateOutput(
        uint64 outputNumber,
        bytes32 outputHash,
        bytes calldata blobCommitment,
        bytes calldata kzgProof
    ) external override returns (bool success) {
        uint256 blobIndex = KailuaLib.blobIndex(outputNumber);
        uint256 blobPosition = KailuaLib.blobPosition(outputNumber);
        bytes32 proposalBlobHash = KailuaLib.versionedKZGHash(blobCommitment);
        require(proposalBlobHash == proposalBlobHashes[blobIndex].raw(), "bad proposalBlobHash");
        success =
            KailuaLib.verifyKZGBlobProof(proposalBlobHash, uint32(blobPosition), outputHash, blobCommitment, kzgProof);
    }

    /// @inheritdoc KailuaTournament
    function getChallengerDuration(uint256 asOfTimestamp) public view override returns (Duration duration_) {
        // INVARIANT: The game must be in progress to query the remaining time to respond to a given claim.
        if (status != GameStatus.IN_PROGRESS) {
            revert GameNotInProgress();
        }

        // Compute the duration elapsed of the potential challenger's clock.
        uint64 elapsed = uint64(asOfTimestamp - createdAt.raw());
        uint64 maximum = MAX_CLOCK_DURATION.raw();
        duration_ = elapsed >= maximum ? Duration.wrap(0) : Duration.wrap(maximum - elapsed);
    }
}
