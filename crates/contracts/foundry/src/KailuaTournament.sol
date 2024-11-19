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
import "./KailuaLib.sol";

abstract contract KailuaTournament is Clone, IDisputeGame {
    // ------------------------------
    // Immutable configuration
    // ------------------------------

    /// @notice The RISC Zero verifier contract
    IRiscZeroVerifier internal immutable RISC_ZERO_VERIFIER;

    /// @notice The RISC Zero image id of the fault proof program
    bytes32 internal immutable FPVM_IMAGE_ID;

    /// @notice The hash of the game configuration
    bytes32 internal immutable GAME_CONFIG_HASH;

    /// @notice The number of blocks a claim must cover
    uint256 internal immutable PROPOSAL_BLOCK_COUNT;

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

    /// @notice Returns the number of blocks that must be covered by this game
    function proposalBlockCount() external view returns (uint256 proposalBlockCount_) {
        proposalBlockCount_ = PROPOSAL_BLOCK_COUNT;
    }

    /// @notice Returns the anchor state registry contract.
    function anchorStateRegistry() external view returns (IAnchorStateRegistry registry_) {
        registry_ = ANCHOR_STATE_REGISTRY;
    }

    constructor(
        IRiscZeroVerifier _verifierContract,
        bytes32 _imageId,
        bytes32 _configHash,
        uint256 _proposalBlockCount,
        GameType _gameType,
        IAnchorStateRegistry _anchorStateRegistry
    ) {
        RISC_ZERO_VERIFIER = _verifierContract;
        FPVM_IMAGE_ID = _imageId;
        GAME_CONFIG_HASH = _configHash;
        PROPOSAL_BLOCK_COUNT = _proposalBlockCount;
        GAME_TYPE = _gameType;
        ANCHOR_STATE_REGISTRY = _anchorStateRegistry;
    }

    // ------------------------------
    // Fault proving
    // ------------------------------

    /// @notice The address of the prover of a fight between children
    mapping(uint256 => mapping(uint256 => address)) public prover;

    /// @notice The timestamp of when the first proof for a fight between children was made
    mapping(uint256 => mapping(uint256 => Timestamp)) public provenAt;

    /// @notice The current proof status of a fight between children
    mapping(uint256 => mapping(uint256 => ProofStatus)) public proofStatus;

    /// @notice The proposals extending this proposal
    KailuaTournament[] public children;

    /// @notice The l2BlockNumber of the claim's output root.
    function l2BlockNumber() public pure returns (uint256 l2BlockNumber_) {
        l2BlockNumber_ = _getArgUint64(0x54);
    }

    function verifyIntermediateOutput(
        uint32 outputNumber,
        bytes32 outputHash,
        bytes calldata blobCommitment,
        bytes calldata kzgProof
    ) external virtual returns (bool success);

    /// @notice Proves the outcome of a tournament match
    function prove(
        uint32[3] calldata uvo,
        bytes calldata encodedSeal,
        bytes32 acceptedOutput,
        bytes32[2] calldata proposedOutput,
        bytes32 computedOutput,
        bytes[2][] calldata blobCommitments,
        bytes[2][] calldata kzgProofs
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
            revert BadExtraData();
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

        // fault => u was shown as faulty
        // bool isFaultProof = proposedOutput[0] != computedOutput;

        // Construct the expected journal
        uint64 claimBlockNumber = uint64(l2BlockNumber() + uvo[2]);
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

    /// @notice Registers a new proposal that extends this one
    function appendChild() external {
        IDisputeGameFactory disputeGameFactory = ANCHOR_STATE_REGISTRY.disputeGameFactory();
        uint256 nonce = ANCHOR_STATE_REGISTRY.disputeGameFactory().gameCount();
        address childAddress = address(bytes20(keccak256(abi.encodePacked(address(disputeGameFactory), nonce))));
        // INVARIANT: The calling contract is a newly deployed contract by the dispute game factory
        if (msg.sender != childAddress) {
            revert BadAuth();
        }

        // Append new child to children list
        children.push(KailuaTournament(msg.sender));

        // INVARIANT: Do not accept further proposals after the first child's timeout
        if (children[0].getChallengerDuration().raw() == 0) {
            revert ClockExpired();
        }

        // todo: automatically request fault proof from boundless to resolve dispute
    }

    /// @notice Eliminates children until at least one remains
    function pruneChildren() external view returns (KailuaTournament survivor) {
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

    /// @notice Returns the amount of time left for challenges.
    function getChallengerDuration() public view virtual returns (Duration duration_);

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
}
