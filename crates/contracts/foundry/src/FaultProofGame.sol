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
import "./ProofLib.sol";

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

    /// @notice The timestamp of the genesis l2 block
    uint256 internal immutable GENESIS_TIME_STAMP;

    /// @notice The time between l2 blocks
    uint256 internal immutable L2_BLOCK_TIME;

    /// @notice The number of blocks a claim must cover
    uint256 internal immutable PROPOSAL_BLOCK_COUNT;

    /// @notice The minimum gap between the l1 and proposed l2 tip timestamps
    uint256 internal immutable PROPOSAL_TIME_GAP;

    /// @notice The number of blobs a claim must provide
    uint256 internal immutable PROPOSAL_BLOBS;

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

    /// @notice Returns the timestamp of the genesis L2 block
    function genesisTimeStamp() external view returns (uint256 genesisTimeStamp_) {
        genesisTimeStamp_ = GENESIS_TIME_STAMP;
    }

    /// @notice Returns the inter-block time of the L2
    function l2BlockTime() external view returns (uint256 l2BlockTime_) {
        l2BlockTime_ = L2_BLOCK_TIME;
    }

    /// @notice Returns the number of blocks that must be covered by this game
    function proposalBlockCount() external view returns (uint256 proposalBlockCount_) {
        proposalBlockCount_ = PROPOSAL_BLOCK_COUNT;
    }

    /// @notice Returns the required gap between the current l1 timestamp and the proposal's l2 timestamp
    function proposalTimeGap() external view returns (uint256 proposalTimeGap_) {
        proposalTimeGap_ = PROPOSAL_TIME_GAP;
    }

    /// @notice Returns the number of blobs containing intermediate blob data
    function proposalBlobs() external view returns (uint256 proposalBlobs_) {
        proposalBlobs_ = PROPOSAL_BLOBS;
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
        IRiscZeroVerifier _verifierContract,
        bytes32 _imageId,
        bytes32 _configHash,
        uint256 _genesisTimeStamp,
        uint256 _l2BlockTime,
        uint256 _proposalBlockCount,
        uint256 _proposalTimeGap,
        Duration _maxClockDuration,
        GameType _gameType,
        IAnchorStateRegistry _anchorStateRegistry
    ) {
        RISC_ZERO_VERIFIER = _verifierContract;
        FPVM_IMAGE_ID = _imageId;
        GAME_CONFIG_HASH = _configHash;
        GENESIS_TIME_STAMP = _genesisTimeStamp;
        L2_BLOCK_TIME = _l2BlockTime;
        PROPOSAL_BLOCK_COUNT = _proposalBlockCount;
        PROPOSAL_TIME_GAP = _proposalTimeGap;
        PROPOSAL_BLOBS = (_proposalBlockCount / (1 << ProofLib.FIELD_ELEMENTS_PER_BLOB_PO2))
            + ((_proposalBlockCount % (1 << ProofLib.FIELD_ELEMENTS_PER_BLOB_PO2)) == 0 ? 0 : 1);
        MAX_CLOCK_DURATION = _maxClockDuration;
        GAME_TYPE = _gameType;
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
        // Expected length: 0x6A
        // - 0x04 selector
        // - 0x14 creator address
        // - 0x20 root claim
        // - 0x20 l1 head
        // - 0x10 extraData (0x08 l2BlockNumber, 0x08 parentGameIndex)
        // - 0x02 CWIA bytes
        if (msg.data.length != 0x6A) {
            revert BadExtraData();
        }

        // Do not allow the game to be initialized if the root claim corresponds to a block at or before the
        // starting block number. (0xf40239db)
        uint256 thisL2BlockNumber = l2BlockNumber();
        uint256 prevL2BlockNumber = startingBlockNumber();
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

        // Record the bonded value
        bond = msg.value;

        // Register this new game in the parent game's contract
        FaultProofGame sibling = parentGame().appendChild();

        if (address(sibling) != address(0x0)) {
            // todo: do not permit conflicting proposals after the timeout period

            // todo: automatically request fault proof from boundless to resolve dispute
        }

        // Do not permit proposals of l2 blocks past the gap
        if (block.timestamp <= GENESIS_TIME_STAMP + thisL2BlockNumber * L2_BLOCK_TIME + PROPOSAL_TIME_GAP) {
            revert ClockTimeExceeded();
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

    /// @inheritdoc IDisputeGame
    function extraData() external pure returns (bytes memory extraData_) {
        // The extra data starts at the second word within the cwia calldata and
        // is 48 bytes long.
        extraData_ = _getArgBytes(0x54, 0x0F);
    }

    /// @inheritdoc IDisputeGame
    function gameData() external view returns (GameType gameType_, Claim rootClaim_, bytes memory extraData_) {
        gameType_ = this.gameType();
        rootClaim_ = this.rootClaim();
        extraData_ = this.extraData();
    }

    /// @inheritdoc IDisputeGame
    function resolve() external returns (GameStatus status_) {
        // INVARIANT: Resolution cannot occur unless the game is currently in progress.
        if (status != GameStatus.IN_PROGRESS) {
            revert GameNotInProgress();
        }

        // INVARIANT: Optimistic resolution cannot occur unless parent game is resolved.
        FaultProofGame parentGame_ = parentGame();
        if (parentGame_.status() != GameStatus.DEFENDER_WINS) {
            revert OutOfOrderResolution();
        }

        // INVARIANT: Cannot resolve unless the clock has expired
        if (getChallengerDuration().raw() < MAX_CLOCK_DURATION.raw()) {
            revert ClockNotExpired();
        }

        // INVARIANT: Can only resolve the last remaining child
        if (parentGame_.pruneChildren() != this) {
            revert OutOfOrderResolution();
        }

        // Refund the proposer
        ProofLib.pay(address(this).balance, gameCreator());

        // Mark resolution timestamp
        resolvedAt = Timestamp.wrap(uint64(block.timestamp));

        // Update the status and emit the resolved event, note that we're performing a storage update here.
        emit Resolved(status = status_ = GameStatus.DEFENDER_WINS);

        // Try to update the anchor state, this should not revert.
        ANCHOR_STATE_REGISTRY.tryUpdateAnchorState();
    }

    // ------------------------------
    // Immutable instance data
    // ------------------------------

    /// @notice The l2BlockNumber of the claim's output root.
    function l2BlockNumber() public pure returns (uint256 l2BlockNumber_) {
        l2BlockNumber_ = _getArgUint64(0x54);
    }

    /// @notice The index of the parent game in the `DisputeGameFactory`.
    function parentGameIndex() public pure returns (uint64 parentGameIndex_) {
        parentGameIndex_ = _getArgUint64(0x5C);
    }

    /// @notice The parent game contract.
    function parentGame() public view returns (FaultProofGame parentGame_) {
        (GameType parentGameType,, IDisputeGame parentDisputeGame) =
            ANCHOR_STATE_REGISTRY.disputeGameFactory().gameAtIndex(parentGameIndex());

        // Only allow fault claim games to be based off of other instances of the same game type
        if (parentGameType.raw() != GAME_TYPE.raw()) revert GameTypeMismatch(parentGameType, GAME_TYPE);

        // Interpret parent game as another instance of this game type
        parentGame_ = FaultProofGame(address(parentDisputeGame));
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
    // Fault proving
    // ------------------------------

    /// @notice The blob hashes used to create the game
    Hash[] public proposalBlobHashes;

    /// @notice The bond paid to initiate the game
    uint256 public bond;

    /// @notice The address of the prover of a fight between children
    mapping(uint256 => mapping(uint256 => address)) public prover;

    /// @notice The timestamp of when the first proof for a fight between children was made
    mapping(uint256 => mapping(uint256 => Timestamp)) public provenAt;

    /// @notice The current proof status of a fight between children
    mapping(uint256 => mapping(uint256 => ProofStatus)) public proofStatus;

    /// @notice The point at which proposals diverge
    mapping(uint256 => uint256) public divergencePoint;

    /// @notice The proposals extending this proposal
    FaultProofGame[] public children;

    /// @notice Registers a new proposal that extends this one
    function appendChild() external returns (FaultProofGame sibling_) {
        IDisputeGameFactory disputeGameFactory = ANCHOR_STATE_REGISTRY.disputeGameFactory();
        uint256 nonce = ANCHOR_STATE_REGISTRY.disputeGameFactory().gameCount();
        address childAddress = address(bytes20(keccak256(abi.encodePacked(address(disputeGameFactory), nonce))));
        // INVARIANT: The calling contract is a newly deployed contract by the dispute game factory
        if (msg.sender != childAddress) {
            revert BadAuth();
        }

        if (children.length > 0) {
            // INVARIANT: Do not accept further proposals after the first child's timeout
            if (children[0].getChallengerDuration().raw() >= MAX_CLOCK_DURATION.raw()) {
                revert ClockExpired();
            }

            // Return possible sibling
            sibling_ = children[children.length - 1];
        }

        // Append new child to children list
        children.push(FaultProofGame(msg.sender));
    }

    /// @notice Eliminates children until at least one remains
    function pruneChildren() external view returns (FaultProofGame survivor) {
        require(children.length > 0);
        uint256 u = 0;
        for (uint256 v = 1; v < children.length; v++) {
            ProofStatus proven = proofStatus[u][v];
            require(proven != ProofStatus.NONE);
            if (proven == ProofStatus.FAULT) {
                // u was shown as faulty
                u = v;
            } else {
                // u survives
            }
        }
        survivor = children[u];
    }

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

    function verifyIntermediateOutput(
        uint32 outputNumber,
        bytes32 outputHash,
        bytes calldata blobCommitment,
        bytes calldata kzgProof
    ) external returns (bool success) {
        uint256 blobIndex = ProofLib.blobIndex(outputNumber);
        bytes32 proposalBlobHash = ProofLib.versionedKZGHash(blobCommitment);
        require(proposalBlobHash == proposalBlobHashes[blobIndex].raw(), "bad proposalBlobHash");
        success = ProofLib.verifyKZGBlobProof(proposalBlobHash, outputNumber - 1, outputHash, blobCommitment, kzgProof);
    }

    /// @notice Proves the integrity or faultiness of the output argued on by this contract
    function prove(
        uint32[3] calldata uvo,
        bytes calldata encodedSeal,
        bytes32 acceptedOutput,
        bytes32[2] calldata proposedOutput,
        bytes32 computedOutput,
        bytes[2][] calldata blobCommitments,
        bytes[2][] calldata kzgProofs
    ) external {
        FaultProofGame[2] memory childContracts = [children[uvo[0]], children[uvo[1]]];
        // INVARIANT: Proofs cannot be submitted unless the children are playing.
        if (childContracts[0].status() != GameStatus.IN_PROGRESS) {
            revert GameNotInProgress();
        }
        if (childContracts[1].status() != GameStatus.IN_PROGRESS) {
            revert GameNotInProgress();
        }

        // INVARIANT: Proofs can only be submitted once
        if (proofStatus[uvo[0]][uvo[1]] != ProofStatus.NONE) {
            revert AlreadyProven();
        }

        // INVARIANT: Proofs can only argue on divergence points
        if (proposedOutput[0] == proposedOutput[1]) {
            revert BadExtraData();
        }

        // Validate the common output root.
        if (uvo[2] == 0) {
            // The safe output is the parent game's output when proving the first output
            require(acceptedOutput == startingRootHash().raw());
        } else {
            // Prove common output publication
            require(
                childContracts[0].verifyIntermediateOutput(
                    uvo[2] - 1, acceptedOutput, blobCommitments[0][0], kzgProofs[0][0]
                ),
                "bad left child acceptedOutput kzg proof"
            );

            require(
                childContracts[1].verifyIntermediateOutput(
                    uvo[2] - 1, acceptedOutput, blobCommitments[1][0], kzgProofs[1][0]
                ),
                "bad right child acceptedOutput kzg proof"
            );
        }

        // Validate the claimed output roots.
        if (uvo[2] == PROPOSAL_BLOCK_COUNT - 1) {
            require(proposedOutput[0] == childContracts[0].rootClaim().raw());
            require(proposedOutput[1] == childContracts[1].rootClaim().raw());
        } else {
            // Prove divergent output publication
            require(
                childContracts[0].verifyIntermediateOutput(
                    uvo[2],
                    proposedOutput[0],
                    blobCommitments[0][blobCommitments[0].length - 1],
                    kzgProofs[0][kzgProofs[0].length - 1]
                ),
                "bad left child proposedOutput kzg proof"
            );

            require(
                childContracts[1].verifyIntermediateOutput(
                    uvo[2],
                    proposedOutput[1],
                    blobCommitments[1][blobCommitments[1].length - 1],
                    kzgProofs[1][kzgProofs[1].length - 1]
                ),
                "bad right child proposedOutput kzg proof"
            );
        }

        // fault => u was shown as faulty
        // bool isFaultProof = proposedOutput[0] != computedOutput;

        // Construct the expected journal
        uint64 claimBlockNumber = uint64(startingBlockNumber() + uvo[2]);
        bytes32 journalDigest = sha256(
            abi.encodePacked(
                // The parent proposal's claim hash
                rootClaim().raw(),
                // The L1 head hash containing the safe L2 chain data that may reproduce the L2 head hash.
                childContracts[1].l1Head().raw(),
                // The latest finalized L2 output root.
                acceptedOutput,
                // The L2 output root claim.
                computedOutput,
                // The L2 claim block number.
                claimBlockNumber,
                // The configuration hash for this game
                GAME_CONFIG_HASH
            )
        );

        // reverts on failure
        RISC_ZERO_VERIFIER.verify(encodedSeal, FPVM_IMAGE_ID, journalDigest);

        // Update proof status
        emit Proven(
            uvo[2],
            proofStatus[uvo[0]][uvo[1]] =
                proposedOutput[0] != computedOutput ? ProofStatus.FAULT : ProofStatus.INTEGRITY
        );

        // Set the game's prover address
        prover[uvo[0]][uvo[1]] = msg.sender;

        // Set the game's proving timestamp
        provenAt[uvo[0]][uvo[1]] = Timestamp.wrap(uint64(block.timestamp));
    }
}
