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

abstract contract KailuaTournament is Clone, IDisputeGame {
    // ------------------------------
    // Immutable configuration
    // ------------------------------

    /// @notice The Kailua Treasury Implementation contract address
    IKailuaTreasury internal immutable KAILUA_TREASURY;

    /// @notice The RISC Zero verifier contract
    IRiscZeroVerifier internal immutable RISC_ZERO_VERIFIER;

    /// @notice The RISC Zero image id of the fault proof program
    bytes32 internal immutable FPVM_IMAGE_ID;

    /// @notice The hash of the game configuration
    bytes32 internal immutable ROLLUP_CONFIG_HASH;

    /// @notice The number of blocks a claim must cover
    uint256 internal immutable PROPOSAL_BLOCK_COUNT;

    /// @notice The number of blobs a claim must provide
    uint256 internal immutable PROPOSAL_BLOBS;

    /// @notice The game type ID
    GameType internal immutable GAME_TYPE;

    /// @notice The dispute game factory
    IDisputeGameFactory internal immutable DISPUTE_GAME_FACTORY;

    /// @notice Returns the address of the Kailua Treasury used by tournament instances
    function treasury() public view returns (IKailuaTreasury treasury_) {
        treasury_ = KAILUA_TREASURY;
    }

    /// @notice Returns the address of the RISC Zero verifier used by this contract
    function verifier() public view returns (IRiscZeroVerifier verifier_) {
        verifier_ = RISC_ZERO_VERIFIER;
    }

    /// @notice Returns the RISC Zero Image ID of the FPVM program used by this contract
    function imageId() public view returns (bytes32 imageId_) {
        imageId_ = FPVM_IMAGE_ID;
    }

    /// @notice Returns the hash of the configuration of this game
    function configHash() public view returns (bytes32 configHash_) {
        configHash_ = ROLLUP_CONFIG_HASH;
    }

    /// @notice Returns the number of blocks that must be covered by this game
    function proposalBlockCount() public view returns (uint256 proposalBlockCount_) {
        proposalBlockCount_ = PROPOSAL_BLOCK_COUNT;
    }

    /// @notice Returns the number of blobs containing intermediate blob data
    function proposalBlobs() public view returns (uint256 proposalBlobs_) {
        proposalBlobs_ = PROPOSAL_BLOBS;
    }

    function disputeGameFactory() public view returns (IDisputeGameFactory factory_) {
        factory_ = DISPUTE_GAME_FACTORY;
    }

    constructor(
        IKailuaTreasury _kailuaTreasury,
        IRiscZeroVerifier _verifierContract,
        bytes32 _imageId,
        bytes32 _configHash,
        uint256 _proposalBlockCount,
        GameType _gameType,
        IDisputeGameFactory _disputeGameFactory
    ) {
        KAILUA_TREASURY = _kailuaTreasury;
        RISC_ZERO_VERIFIER = _verifierContract;
        FPVM_IMAGE_ID = _imageId;
        ROLLUP_CONFIG_HASH = _configHash;
        PROPOSAL_BLOCK_COUNT = _proposalBlockCount;
        PROPOSAL_BLOBS = (_proposalBlockCount / (1 << KailuaLib.FIELD_ELEMENTS_PER_BLOB_PO2))
            + ((_proposalBlockCount % (1 << KailuaLib.FIELD_ELEMENTS_PER_BLOB_PO2)) == 0 ? 0 : 1);
        GAME_TYPE = _gameType;
        DISPUTE_GAME_FACTORY = _disputeGameFactory;
    }

    /// @notice The blob hashes used to create the game
    Hash[] public proposalBlobHashes;

    function initializeInternal() internal {
        // INVARIANT: The game must not have already been initialized.
        if (createdAt.raw() > 0) revert AlreadyInitialized();

        // Set the game's starting timestamp
        createdAt = Timestamp.wrap(uint64(block.timestamp));

        // Set the game's index in the factory
        gameIndex = disputeGameFactory().gameCount();
    }

    // ------------------------------
    // Fault proving
    // ------------------------------

    /// @notice The game's index in the factory
    uint256 public gameIndex;

    /// @notice The address of the prover of a fight between children
    mapping(uint256 => mapping(uint256 => address)) public prover;

    /// @notice The timestamp of when the first proof for a fight between children was made
    mapping(uint256 => mapping(uint256 => Timestamp)) public provenAt;

    /// @notice The current proof status of a fight between children
    mapping(uint256 => mapping(uint256 => ProofStatus)) public proofStatus;

    /// @notice The proposals extending this proposal
    KailuaTournament[] public children;

    function verifyIntermediateOutput(
        uint64 outputNumber,
        bytes32 outputHash,
        bytes calldata blobCommitment,
        bytes calldata kzgProof
    ) external virtual returns (bool success);

    /// @notice Proves the outcome of a tournament match
    function prove(
        uint64[3] calldata uvo,
        bytes calldata encodedSeal,
        bytes32 acceptedOutput,
        bytes32[2] calldata proposedOutput,
        bytes32 computedOutput,
        bytes[][2] calldata blobCommitments,
        bytes[][2] calldata kzgProofs
    ) external {
        KailuaTournament[2] memory childContracts = [children[uvo[0]], children[uvo[1]]];
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
            revert NoConflict();
        }

        bytes32 preconditionHash = bytes32(0x0);
        // INVARIANT: Published data is equivalent until divergent blob
        if (uvo[2] > 0) {
            // Find the divergent blob index
            uint256 divergentBlobIndex = KailuaLib.blobIndex(uvo[2]);
            if (uvo[2] == PROPOSAL_BLOCK_COUNT - 1) {
                // If the only difference is the root claim, require all blobs to be equal.
                divergentBlobIndex = PROPOSAL_BLOBS;
            }
            // Ensure blob hashes are equal until divergence
            for (uint256 i = 0; i < divergentBlobIndex; i++) {
                if (childContracts[0].proposalBlobHashes(i).raw() != childContracts[1].proposalBlobHashes(i).raw()) {
                    revert BlobHashMismatch(
                        childContracts[0].proposalBlobHashes(i).raw(), childContracts[1].proposalBlobHashes(i).raw()
                    );
                }
            }
            // Update required precondition hash from proof if not at a boundary
            if (KailuaLib.blobPosition(uvo[2]) != 0 && uvo[2] < PROPOSAL_BLOCK_COUNT - 1) {
                preconditionHash = sha256(
                    abi.encodePacked(
                        childContracts[0].proposalBlobHashes(divergentBlobIndex).raw(),
                        childContracts[1].proposalBlobHashes(divergentBlobIndex).raw()
                    )
                );
            }
        }

        // Validate the common output root.
        if (uvo[2] == 0) {
            // The safe output is the parent game's output when proving the first output
            require(acceptedOutput == rootClaim().raw());
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

        {
            // Construct the expected journal
            uint64 claimBlockNumber = uint64(l2BlockNumber() + uvo[2] + 1);
            bytes32 journalDigest = sha256(
                abi.encodePacked(
                    // The parent proposal's claim hash
                    preconditionHash,
                    // The L1 head hash containing the safe L2 chain data that may reproduce the L2 head hash.
                    childContracts[1].l1Head().raw(),
                    // The latest finalized L2 output root.
                    acceptedOutput,
                    // The L2 output root claim.
                    computedOutput,
                    // The L2 claim block number.
                    claimBlockNumber,
                    // The configuration hash for this game
                    ROLLUP_CONFIG_HASH
                )
            );

            // reverts on failure
            RISC_ZERO_VERIFIER.verify(encodedSeal, FPVM_IMAGE_ID, journalDigest);
        }

        // Update proof status based on normalized hashes
        if (KailuaLib.hashToFe(proposedOutput[0]) != KailuaLib.hashToFe(computedOutput)) {
            // u lose
            if (KailuaLib.hashToFe(proposedOutput[1]) != KailuaLib.hashToFe(computedOutput)) {
                // v lose
                proofStatus[uvo[0]][uvo[1]] = ProofStatus.U_LOSE_V_LOSE;
            } else {
                // v win
                proofStatus[uvo[0]][uvo[1]] = ProofStatus.U_LOSE_V_WIN;
            }
        } else {
            // u win
            proofStatus[uvo[0]][uvo[1]] = ProofStatus.U_WIN_V_LOSE;
        }

        emit Proven(uvo[0], uvo[1], proofStatus[uvo[0]][uvo[1]]);

        // Set the game's prover address
        prover[uvo[0]][uvo[1]] = msg.sender;

        // Set the game's proving timestamp
        provenAt[uvo[0]][uvo[1]] = Timestamp.wrap(uint64(block.timestamp));
    }

    /// @notice Registers a new proposal that extends this one
    function appendChild() external {
        // INVARIANT: The calling contract is a newly deployed contract by the dispute game factory
        if (!KAILUA_TREASURY.isProposing()) {
            revert UnknownGame();
        }

        // INVARIANT: The calling KailuaGame contract is not referring to itself as a parent
        if (msg.sender == address(this)) {
            revert InvalidParent();
        }

        // Append new child to children list
        children.push(KailuaTournament(msg.sender));
        // todo: automatically request fault proof from boundless to resolve dispute
    }

    /// @notice Eliminates children until at least one remains
    // todo: this needs to be refactored into a tape-style function that can be resumed in case gas is high
    function pruneChildren() external returns (KailuaTournament survivor) {
        // INVARIANT: Only finalized proposals may host tournaments
        if (status != GameStatus.DEFENDER_WINS) {
            revert GameNotResolved();
        }

        // INVARIANT: No tournament to play without at least one child
        if (children.length == 0) {
            revert NotProposed();
        }

        // Select the first possible survivor
        uint256 u;
        for (u = 0; u < children.length; u++) {
            if (!isChildEliminated(children[u])) {
                break;
            }
        }
        // Eliminate other opponents
        uint256 v;
        for (v = u + 1; v < children.length; v++) {
            KailuaTournament contender = children[u];
            KailuaTournament opponent = children[v];
            // If the opponent is eliminated or has the same identity, skip
            if (canIgnoreOpponent(contender, opponent)) {
                continue;
            }
            // If the survivor hasn't been challenged for as long as the timeout, declare them winner
            if (contender.getChallengerDuration(opponent.createdAt().raw()).raw() == 0) {
                break;
            }
            // If the opponent proposal is a twin, skip it
            if (contender.rootClaim().raw() == opponent.rootClaim().raw()) {
                uint256 common;
                for (common = 0; common < PROPOSAL_BLOBS; common++) {
                    if (contender.proposalBlobHashes(common).raw() != opponent.proposalBlobHashes(common).raw()) {
                        break;
                    }
                }
                if (common == PROPOSAL_BLOBS) {
                    // The opponent is an unjustified duplicate. Ignore it.
                    continue;
                }
            }
            // Check if the result of playing this match is available
            ProofStatus proven = proofStatus[u][v];
            // We must wait for more proofs if the result is unavailable
            require(proven != ProofStatus.NONE);
            // Otherwise decide winner
            if (proven == ProofStatus.U_LOSE_V_WIN) {
                // u was shown as faulty (beat by v)
                // eliminate the player
                KAILUA_TREASURY.eliminate(address(contender), prover[u][v]);
                // proceed with opponent as new player
                u = v;
            } else {
                // assume u survives
                // todo jump over u if both players lose?
                // eliminate the opponent
                KAILUA_TREASURY.eliminate(address(opponent), prover[u][v]);
                // proceed with the same player
            }
        }
        // todo: Handle stragglers?
        // Return the sole survivor
        survivor = children[u];
    }

    function isChildEliminated(KailuaTournament child) internal returns (bool) {
        address _proposer = KAILUA_TREASURY.proposerOf(address(child));
        uint256 eliminationRound = KAILUA_TREASURY.eliminationRound(_proposer);
        if (eliminationRound == 0 || eliminationRound > child.gameIndex()) {
            // This proposer has not been eliminated as of their proposal at gameIndex
            return false;
        }
        return true;
    }

    function canIgnoreOpponent(KailuaTournament contender, KailuaTournament opponent) internal returns (bool) {
        address opponentProposer = KAILUA_TREASURY.proposerOf(address(opponent));
        uint256 eliminationRound = KAILUA_TREASURY.eliminationRound(opponentProposer);
        if (eliminationRound == 0 || eliminationRound > opponent.gameIndex()) {
            address contenderProposer = KAILUA_TREASURY.proposerOf(address(contender));
            // The opponent is fighting itself
            return contenderProposer == opponentProposer;
        }
        // The opponent had been eliminated prior to their proposal
        return true;
    }

    /// @notice Returns the amount of time left for challenges as of the input timestamp.
    function getChallengerDuration(uint256 asOfTimestamp) public view virtual returns (Duration duration_);

    /// @notice Returns the parent game contract.
    function parentGame() public view virtual returns (KailuaTournament parentGame_);

    /// @notice Returns the proposer address
    function proposer() public returns (address proposer_) {
        proposer_ = KAILUA_TREASURY.proposerOf(address(this));
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

    /// @notice The l2BlockNumber of the claim's output root.
    function l2BlockNumber() public pure returns (uint256 l2BlockNumber_) {
        l2BlockNumber_ = uint256(_getArgUint64(0x54));
    }

    /// @inheritdoc IDisputeGame
    function gameData() external view returns (GameType gameType_, Claim rootClaim_, bytes memory extraData_) {
        gameType_ = this.gameType();
        rootClaim_ = this.rootClaim();
        extraData_ = this.extraData();
    }
}
