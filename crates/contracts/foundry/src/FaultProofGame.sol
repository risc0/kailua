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

contract FaultProofGame is Clone, IDisputeGame {
    /// @notice Semantic version.
    /// @custom:semver 0.1.0
    string public constant version = "0.1.0";

    // ------------------------------
    // Immutable configuration
    // ------------------------------

    /// @notice The RISC Zero verifier contract
    IRiscZeroVerifier internal immutable RISC_ZERO_VERIFIER;

    /// @notice The RISC Zero image id of the fault proof program
    bytes32 internal immutable FPVM_IMAGE_ID;

    /// @notice The hash of the game configuration
    bytes32 internal immutable GAME_CONFIG_HASH;

    /// @notice The maximum number of blocks a claim may cover
    uint256 internal immutable MAX_BLOCK_COUNT;

    /// @notice The duration after which the proposal is accepted
    Duration internal immutable MAX_CLOCK_DURATION;

    /// @notice The game type ID
    GameType internal immutable GAME_TYPE;

    /// @notice The anchor state registry.
    IAnchorStateRegistry internal immutable ANCHOR_STATE_REGISTRY;

    /// @notice Returns the address of the RISC Zero verifier used by this contract
    function verifier() external view returns (IRiscZeroVerifier verifier_) {
        verifier_ = RISC_ZERO_VERIFIER;
    }

    /// @notice Returns the RISC Zero Image ID of the FPVM program used by this contract
    function imageId() external view returns (bytes32 imageId_) {
        imageId_ = FPVM_IMAGE_ID;
    }

    /// @notice Returns the hash of the configuration of this game
    function configHash() external view returns (bytes32 configHash_) {
        configHash_ = GAME_CONFIG_HASH;
    }

    /// @notice Returns the maximum number of blocks that can be covered by this game
    function maxBlockCount() external view returns (uint256 maxBlockCount_) {
        maxBlockCount_ = MAX_BLOCK_COUNT;
    }

    /// @notice Returns the max clock duration.
    function maxClockDuration() external view returns (Duration maxClockDuration_) {
        maxClockDuration_ = MAX_CLOCK_DURATION;
    }

    /// @notice Returns the game type.
    function gameType() external view returns (GameType gameType_) {
        gameType_ = GAME_TYPE;
    }

    /// @notice Returns the anchor state registry contract.
    function anchorStateRegistry() external view returns (IAnchorStateRegistry registry_) {
        registry_ = ANCHOR_STATE_REGISTRY;
    }

    constructor(
        IRiscZeroVerifier _verifierContract,
        bytes32 _imageId,
        bytes32 _configHash,
        uint256 _maxBlockCount,
        Duration _maxClockDuration,
        GameType _gameType,
        IAnchorStateRegistry _anchorStateRegistry
    ) {
        RISC_ZERO_VERIFIER = _verifierContract;
        FPVM_IMAGE_ID = _imageId;
        GAME_CONFIG_HASH = _configHash;
        GAME_TYPE = _gameType;
        MAX_BLOCK_COUNT = _maxBlockCount;
        MAX_CLOCK_DURATION = _maxClockDuration;
        ANCHOR_STATE_REGISTRY = _anchorStateRegistry;
    }

    /// @notice The starting timestamp of the game
    Timestamp public createdAt;

    /// @notice The bond paid to initiate the game
    uint256 public bond;

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
        // Expected length: 0x8A
        // - 0x04 selector
        // - 0x14 creator address
        // - 0x20 root claim
        // - 0x20 l1 head
        // - 0x30 extraData (0x08 l2BlockNumber, 0x08 parentGameIndex, 0x20 proposalBlobHash)
        // - 0x02 CWIA bytes
        assembly {
            if iszero(eq(calldatasize(), 0x8A)) {
                // Store the selector for `BadExtraData()` & revert
                mstore(0x00, 0x9824bdab)
                revert(0x1C, 0x04)
            }
        }

        // Do not allow the game to be initialized if the root claim corresponds to a block at or before the
        // starting block number. (0xf40239db)
        if (l2BlockNumber() <= startingBlockNumber()) revert UnexpectedRootClaim(rootClaim());

        // Do not initialize a game that covers more blocks than permitted
        if (l2BlockNumber() - startingBlockNumber() > MAX_BLOCK_COUNT) {
            revert BlockCountExceeded(l2BlockNumber(), startingBlockNumber());
        }

        // Validate the intermediate output blob hash
        if (blobhash(0) != proposalBlobHash().raw()) {
            revert BlobHashMismatch(proposalBlobHash().raw(), blobhash(0));
        }

        // Record the bonded value
        bond = msg.value;

        // Set the game's starting timestamp
        createdAt = Timestamp.wrap(uint64(block.timestamp));
    }

    // ------------------------------
    // Initialization data helpers
    // ------------------------------

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

    /// @notice The l2BlockNumber of the claim's output root.
    function l2BlockNumber() public pure returns (uint256 l2BlockNumber_) {
        l2BlockNumber_ = _getArgUint64(0x54);
    }

    /// @notice The index of the parent game in the `DisputeGameFactory`.
    function parentGameIndex() public pure returns (uint64 parentGameIndex_) {
        parentGameIndex_ = _getArgUint64(0x5C);
    }

    /// @notice The hash of the blob of intermediate outputs posted along the proposal.
    function proposalBlobHash() public pure returns (Hash blobHash_) {
        blobHash_ = Hash.wrap(_getArgBytes32(0x64));
    }

    /// @notice Getter for the extra data.
    /// @dev `clones-with-immutable-args` argument #2
    /// @return extraData_ Any extra data supplied to the dispute game contract by the creator.
    function extraData() external pure returns (bytes memory extraData_) {
        // The extra data starts at the second word within the cwia calldata and
        // is 48 bytes long.
        extraData_ = _getArgBytes(0x54, 0x0F);
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
    // Fault proving
    // ------------------------------

    /// @inheritdoc IDisputeGame
    Timestamp public resolvedAt;

    /// @notice The timestamp of each claim's status.
    mapping(uint32 => GameStatus) public gameStatus;

    /// @inheritdoc IDisputeGame
    function status() public view returns (GameStatus status_) {
        status_ = gameStatus[0];
    }

    /// @notice The timestamp of when the first claim against an output was made
    mapping(uint32 => Timestamp) public challengedAt;

    /// @notice The timestamp of when the first proof for an output was made
    mapping(uint32 => Timestamp) public provenAt;

    /// @notice The current proof status of an output.
    mapping(uint32 => ProofStatus) public proofStatus;

    /// @notice The game challenger address
    mapping(uint32 => address) public challenger;

    /// @notice The game prover address
    mapping(uint32 => address) public prover;

    /// @notice The number of unproven challenges made
    uint32 public unresolvedClaimCount;

    /// @notice The number of the first output proven faulty
    uint32 public faultOutputNumber;

    /// @notice Challenges the proposed output to prevent it from being resolved until proven
    function challenge(uint32 outputNumber) external payable {
        // INVARIANT: Proofs cannot be submitted unless the game is currently in progress.
        if (status() != GameStatus.IN_PROGRESS) revert GameNotInProgress();

        // INVARIANT: Proofs can only be submitted once for an output
        if (proofStatus[outputNumber] != ProofStatus.NONE) revert AlreadyProven();

        // INVARIANT: Only the first challenger is accepted for an output if no fault is shown
        if (challenger[outputNumber] != address(0x0)) revert AlreadyChallenged();

        // INVARIANT: The `msg.value` must exactly equal the required bond.
        if (getRequiredBond() != msg.value) revert IncorrectBondAmount();

        // INVARIANT: The challenge must be against a proposed output.
        if (outputNumber > l2BlockNumber() - startingBlockNumber()) {
            revert NotProposed();
        }

        // Set the output challenger address
        emit Challenged(outputNumber, challenger[outputNumber] = payable(msg.sender));

        // Set the output's challenge timestamp
        challengedAt[outputNumber] = Timestamp.wrap(uint64(block.timestamp));

        // Increment the number of unresolved claims
        if (outputNumber > 0) {
            unresolvedClaimCount++;
        }
    }

    /// @notice Proves the integrity or faultiness of the output argued on by this contract
    function prove(
        uint32 outputNumber,
        bytes calldata encodedSeal,
        bytes32 safeOutput,
        bytes32 proposedOutput,
        bytes32 computedOutput,
        bytes calldata blobCommitment, // todo: make this part of init vars
        bytes[] calldata kzgProofs
    ) public {
        // INVARIANT: Proofs cannot be submitted unless the claim's game is currently in progress.
        if (gameStatus[outputNumber] != GameStatus.IN_PROGRESS) revert GameNotInProgress();

        // INVARIANT: Proofs can only be submitted once
        if (proofStatus[outputNumber] != ProofStatus.NONE) revert AlreadyProven();

        // INVARIANT: Can only submit proofs for challenged games
        if (challenger[outputNumber] == address(0x0)) revert UnchallengedGame();

        // The latest finalized L2 output root.
        if (outputNumber <= 1) {
            // The safe output is the parent game's output when proving either
            // 1. The entire game   (outputNumber 0) (todo)
            // 2. The first output  (outputNumber 1)
            require(safeOutput == startingRootHash().raw());
        } else if (outputNumber > 1) {
            // When challenging another output, we must prove that we are using the
            // proposed intermediate output as the parent
            // todo: support empty output compression
            bool success = ProofLib.verifyKZGBlobProof(
                proposalBlobHash().raw(), outputNumber - 2, safeOutput, blobCommitment, kzgProofs[0]
            );
            require(success, "bad safeOutput kzg proof");
        }

        // The claimed output root
        if (outputNumber == l2BlockNumber() - startingBlockNumber()) {
            // The safe output is the entire game's output when proving
            // the last output  (outputNumber N)
            require(proposedOutput == rootClaim().raw());
        } else {
            bool success = ProofLib.verifyKZGBlobProof(
                proposalBlobHash().raw(),
                outputNumber - 1,
                proposedOutput,
                blobCommitment,
                kzgProofs[kzgProofs.length - 1]
            );
            require(success, "bad proposedOutput kzg proof");
        }
        bool isFaultProof = proposedOutput != computedOutput;

        // Construct the expected journal
        bytes32 journalDigest = sha256(
            abi.encodePacked(
                // The L1 head hash containing the safe L2 chain data that may reproduce the L2 head hash.
                l1Head().raw(),
                // The latest finalized L2 output root.
                safeOutput,
                // The L2 output root claim.
                computedOutput,
                // The L2 claim block number.
                uint64(startingBlockNumber() + outputNumber),
                // The configuration hash for this game
                GAME_CONFIG_HASH
            )
        );

        // reverts on failure
        RISC_ZERO_VERIFIER.verify(encodedSeal, FPVM_IMAGE_ID, journalDigest);

        // Update proof status
        emit Proven(outputNumber, proofStatus[outputNumber] = isFaultProof ? ProofStatus.FAULT : ProofStatus.INTEGRITY);

        // Set the game's prover address
        prover[outputNumber] = msg.sender;

        // Set the game's proving timestamp
        provenAt[outputNumber] = Timestamp.wrap(uint64(block.timestamp));

        // Set the game's first faulty output
        if (isFaultProof && faultOutputNumber == 0) {
            faultOutputNumber = outputNumber;
        }
    }

    /// @notice Resolves a dispute on a specific claim based on its presented proof
    function resolveClaim(uint32 outputNumber) external returns (GameStatus status_) {
        // INVARIANT: Resolution cannot occur unless the claim's game is currently in progress.
        if (gameStatus[outputNumber] != GameStatus.IN_PROGRESS) revert GameNotInProgress();

        // INVARIANT: Resolution cannot occur unless a proof is presented for the claim
        ProofStatus proofStatus_ = proofStatus[outputNumber];
        if (proofStatus_ == ProofStatus.NONE) revert NotProven();

        // Resolve based on proof status
        if (proofStatus_ == ProofStatus.INTEGRITY) {
            // Mark the game status in favor of the defender
            status_ = GameStatus.DEFENDER_WINS;

            // Pay the challenger's bond to the prover as compensation
            ProofLib.pay(getRequiredBond(), prover[outputNumber]);
        } else {
            status_ = GameStatus.CHALLENGER_WINS;

            // Refund the challenger's bond
            ProofLib.pay(getRequiredBond(), challenger[outputNumber]);
        }

        // Update game status in storage
        gameStatus[outputNumber] = status_;

        // Decrement the number of unresolved claims
        unresolvedClaimCount--;
    }

    /// @inheritdoc IDisputeGame
    function resolve() external returns (GameStatus status_) {
        // INVARIANT: Resolution cannot occur unless the game is currently in progress.
        if (status() != GameStatus.IN_PROGRESS) revert GameNotInProgress();

        GameStatus parentGameStatus = parentGame().status();
        // Resolve based on :
        // 1. Invalid parent game
        // 2. Fault proof
        // 3. Clock timeout
        if (parentGameStatus == GameStatus.CHALLENGER_WINS) {
            // Set the bond recipient as the base game challenger
            address bondRecipient = challenger[0];

            // Revert if no base game challenger
            if (bondRecipient == address(0x0)) revert UnchallengedGame();

            // Set the status in favor of the challenger
            status_ = GameStatus.CHALLENGER_WINS;

            // Pay the base game challenger
            ProofLib.pay(address(this).balance, bondRecipient);
        } else if (faultOutputNumber != 0) {
            // Set the status in favor of the challenger
            status_ = GameStatus.CHALLENGER_WINS;

            // Pay the first fault prover
            ProofLib.pay(bond, challenger[faultOutputNumber]);
        } else {
            // INVARIANT: Optimistic resolution cannot occur unless parent game and intermediate claims are resolved.
            if (parentGameStatus == GameStatus.IN_PROGRESS || unresolvedClaimCount > 0) revert OutOfOrderResolution();

            // INVARIANT: Cannot resolve an unproven game unless the clock of its would-be proof has expired
            if (getChallengerDuration().raw() < MAX_CLOCK_DURATION.raw()) revert ClockNotExpired();

            // Optimistically resolve in favor of the proposer
            status_ = GameStatus.DEFENDER_WINS;

            // Refund the proposer
            ProofLib.pay(address(this).balance, gameCreator());
        }

        // Mark resolution timestamp
        resolvedAt = Timestamp.wrap(uint64(block.timestamp));

        // Update the status and emit the resolved event, note that we're performing a storage update here.
        emit Resolved(gameStatus[0] = status_);

        // Try to update the anchor state, this should not revert.
        if (status_ == GameStatus.DEFENDER_WINS) {
            ANCHOR_STATE_REGISTRY.tryUpdateAnchorState();
        }
    }

    // ------------------------------
    // Utility methods
    // ------------------------------

    /// @notice The parent game contract.
    function parentGame() public view returns (FaultProofGame parentGame_) {
        (GameType parentGameType,, IDisputeGame parentDisputeGame) =
            ANCHOR_STATE_REGISTRY.disputeGameFactory().gameAtIndex(parentGameIndex());

        // Only allow fault claim games to be based off of other instances of the same game type
        if (parentGameType.raw() != GAME_TYPE.raw()) revert GameTypeMismatch(parentGameType, GAME_TYPE);

        // Interpret parent game as another instance of this game type
        parentGame_ = FaultProofGame(address(parentDisputeGame));
    }

    /// @notice Returns the amount of time elapsed on the potential challenger to the claim's chess clock. Maxes
    ///         out at `MAX_CLOCK_DURATION`.
    /// @return duration_ The time elapsed on the potential challenger to `_claimIndex`'s chess clock.
    function getChallengerDuration() public view returns (Duration duration_) {
        // INVARIANT: The game must be in progress to query the remaining time to respond to a given claim.
        if (gameStatus[0] != GameStatus.IN_PROGRESS) {
            revert GameNotInProgress();
        }

        // Compute the duration elapsed of the potential challenger's clock.
        uint64 challengeDuration = uint64(block.timestamp - createdAt.raw());
        duration_ = challengeDuration > MAX_CLOCK_DURATION.raw() ? MAX_CLOCK_DURATION : Duration.wrap(challengeDuration);
    }

    /// @notice Only the starting block number of the game.
    function startingBlockNumber() public view returns (uint256 startingBlockNumber_) {
        startingBlockNumber_ = parentGame().l2BlockNumber();
    }

    /// @notice Only the starting output root of the game.
    function startingRootHash() public view returns (Hash startingRootHash_) {
        startingRootHash_ = Hash.wrap(parentGame().rootClaim().raw());
    }

    /// @notice Returns the required bond for a challenge.
    /// @return requiredBond_ The required ETH bond in wei.
    function getRequiredBond() public view returns (uint256 requiredBond_) {
        // 2x cheaper to challenge than propose, where x is worst case proving cost
        // challenger of bad proposal will double their money
        // prover of good proposal will be repaid for the validity proof
        requiredBond_ = bond / 2;
    }
}
