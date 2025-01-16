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

    /// @notice The number of outputs a proposal must publish
    uint256 internal immutable PROPOSAL_OUTPUT_COUNT;

    /// @notice The number of blocks each output must cover
    uint256 internal immutable OUTPUT_BLOCK_SPAN;

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

    /// @notice Returns the number of outputs that were published with this game
    function proposalOutputCount() public view returns (uint256 proposalOutputCount_) {
        proposalOutputCount_ = PROPOSAL_OUTPUT_COUNT;
    }

    /// @notice Returns the number of blocks covered by each output in this game
    function outputBlockSpan() public view returns (uint256 outputBlockSpan_) {
        outputBlockSpan_ = OUTPUT_BLOCK_SPAN;
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
        uint256 _proposalOutputCount,
        uint256 _outputBlockSpan,
        GameType _gameType,
        IDisputeGameFactory _disputeGameFactory
    ) {
        KAILUA_TREASURY = _kailuaTreasury;
        RISC_ZERO_VERIFIER = _verifierContract;
        FPVM_IMAGE_ID = _imageId;
        ROLLUP_CONFIG_HASH = _configHash;
        PROPOSAL_OUTPUT_COUNT = _proposalOutputCount;
        OUTPUT_BLOCK_SPAN = _outputBlockSpan;
        PROPOSAL_BLOBS = (_proposalOutputCount / (1 << KailuaLib.FIELD_ELEMENTS_PER_BLOB_PO2))
            + ((_proposalOutputCount % (1 << KailuaLib.FIELD_ELEMENTS_PER_BLOB_PO2)) == 0 ? 0 : 1);
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
    // Tournament
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

    /// @notice The last surviving contender
    uint64 public contenderIndex;

    /// @notice The next unprocessed opponent
    uint64 public opponentIndex;

    /// @notice The next output accepted through a validity proof
    bytes32 public validChildRootClaim;

    /// @notice The next blobs hash accepted through a validity proof
    bytes32 public validChildBlobsHash;

    /// @notice Returns the number of children
    function childCount() external view returns (uint256 count_) {
        count_ = children.length;
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
    }

    /// @notice Eliminates children until at least one remains
    function pruneChildren(uint256 eliminationLimit) external returns (KailuaTournament survivor) {
        // INVARIANT: Only finalized proposals may host tournaments
        if (status != GameStatus.DEFENDER_WINS) {
            revert GameNotResolved();
        }

        // INVARIANT: No tournament to play without at least one child
        if (children.length == 0) {
            revert NotProposed();
        }

        // Discover survivor using available proofs
        if (provenAt[0][0].raw() > 0) {
            survivor = pruneWithValidityProof(eliminationLimit);
        } else {
            survivor = pruneWithFaultProofs(eliminationLimit);
        }
    }

    /// @notice Eliminates children until at least one remains if a validity proof is available
    function pruneWithValidityProof(uint256 eliminationLimit) internal returns (KailuaTournament survivor) {
        // Resume from last surviving contender
        uint64 u = contenderIndex;
        for (; u < children.length && eliminationLimit > 0; (u++, eliminationLimit--)) {
            KailuaTournament child = children[u];
            if (!isChildEliminated(child)) {
                // Find the first viable proven child
                if (child.blobsHash() == validChildBlobsHash && child.rootClaim().raw() == validChildRootClaim) {
                    break;
                }
                // Eliminate faulty proposal
                KAILUA_TREASURY.eliminate(address(child), prover[0][0]);
            }
        }

        // Resume from last unprocessed opponent
        uint64 v = opponentIndex;
        if (v <= u) {
            // Select first possible opponent
            v = u + 1;
        }

        // Eliminate faulty opponents
        KailuaTournament contender = children[u];
        for (; v < children.length && eliminationLimit > 0; (v++, eliminationLimit--)) {
            KailuaTournament opponent = children[v];
            // If the contender hasn't been challenged for as long as the timeout, declare them winner
            if (contender.getChallengerDuration(opponent.createdAt().raw()).raw() == 0) {
                // Note: This implies eliminationLimit > 0
                break;
            }
            // If the opponent proposer is eliminated or has the same identity, skip
            if (canIgnoreOpponent(contender, opponent)) {
                continue;
            }
            // eliminate the opponent
            if (provenAt[0][0].raw() < provenAt[u][v].raw()) {
                KAILUA_TREASURY.eliminate(address(opponent), prover[0][0]);
            } else {
                KAILUA_TREASURY.eliminate(address(opponent), prover[u][v]);
            }
        }

        // INVARIANT: v > u && contender == children[u]
        // Record incremental progress
        contenderIndex = u;
        opponentIndex = v;

        // Return the sole survivor if no more matches can be played
        if (v == children.length || eliminationLimit > 0) {
            survivor = contender;
        }
    }

    /// @notice Eliminates children until at least one remains if all required fault proofs are available
    function pruneWithFaultProofs(uint256 eliminationLimit) internal returns (KailuaTournament survivor) {
        // Resume from last surviving contender
        uint64 u = contenderIndex;
        if (u == 0) {
            // Select the first possible contender
            for (; u < children.length && eliminationLimit > 0; (u++, eliminationLimit--)) {
                if (!isChildEliminated(children[u])) {
                    break;
                }
            }
        }

        // Resume from last unprocessed opponent
        uint64 v = opponentIndex;
        if (v == 0) {
            // Select first possible opponent
            v = u + 1;
        }

        // Match contenders and opponents
        KailuaTournament contender = children[u];
        for (; v < children.length && eliminationLimit > 0; (v++, eliminationLimit--)) {
            KailuaTournament opponent = children[v];
            // If the contender hasn't been challenged for as long as the timeout, declare them winner
            if (contender.getChallengerDuration(opponent.createdAt().raw()).raw() == 0) {
                // Note: This implies eliminationLimit > 0
                break;
            }
            // If the opponent proposer is eliminated, duplicated, or has the same identity, skip
            if (canIgnoreOpponent(contender, opponent)) {
                continue;
            }
            // Check if the result of playing this match is available
            ProofStatus faultProven = proofStatus[u][v];
            // We must wait for more proofs if the result is unavailable
            if (faultProven == ProofStatus.NONE) {
                revert NotProven();
            }
            // Otherwise decide winner
            if (faultProven == ProofStatus.U_LOSE_V_WIN) {
                // u was shown as faulty (beat by v)
                // eliminate the contender
                KAILUA_TREASURY.eliminate(address(contender), prover[u][v]);
                // proceed with opponent as new contender
                u = v;
                contender = opponent;
            } else {
                // assume u survives (todo eliminate u on lose-lose)
                // eliminate the opponent
                KAILUA_TREASURY.eliminate(address(opponent), prover[u][v]);
                // proceed with the same contender
            }
        }

        // INVARIANT: v > u && contender == children[u]
        // Record incremental progress
        contenderIndex = u;
        opponentIndex = v;

        // Return the sole survivor if no more matches can be played
        if (v == children.length || eliminationLimit > 0) {
            survivor = contender;
        }
    }

    /// @notice Returns true iff the child proposal was eliminated
    function isChildEliminated(KailuaTournament child) internal returns (bool) {
        address _proposer = KAILUA_TREASURY.proposerOf(address(child));
        uint256 eliminationRound = KAILUA_TREASURY.eliminationRound(_proposer);
        if (eliminationRound == 0 || eliminationRound > child.gameIndex()) {
            // This proposer has not been eliminated as of their proposal at gameIndex
            return false;
        }
        return true;
    }

    /// @notice Returns true if the opposing proposal can be ignored by the contender
    function canIgnoreOpponent(KailuaTournament contender, KailuaTournament opponent) internal returns (bool) {
        // If the opponent proposal is an identical twin, skip it
        if (contender.rootClaim().raw() == opponent.rootClaim().raw()) {
            // The equivalence of intermediate output commitments matters because one proposal
            // may be more defensible than the other based on the io data.
            if (contender.blobsHash() == opponent.blobsHash()) {
                // The opponent is an unjustified duplicate proposal. Ignore it.
                return true;
            }
        }
        // Check proposer identity
        address opponentProposer = KAILUA_TREASURY.proposerOf(address(opponent));
        uint256 eliminationRound = KAILUA_TREASURY.eliminationRound(opponentProposer);
        // The opponent is not yet eliminated
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
    // Validity proving
    // ------------------------------

    /// @notice Returns the hash of all blob hashes associated with this proposal
    function blobsHash() public view returns (bytes32 blobsHash_) {
        blobsHash_ = sha256(abi.encodePacked(proposalBlobHashes));
    }

    /// @notice Proves that a proposal is valid
    function proveValidity(address payoutRecipient, uint64 childIndex, bytes calldata encodedSeal) external {
        KailuaTournament childContract = children[childIndex];
        // INVARIANT: Can only proof unresolved proposals
        if (childContract.status() != GameStatus.IN_PROGRESS) {
            revert GameNotInProgress();
        }

        // INVARIANT: Proofs can only be submitted once
        if (provenAt[0][0].raw() != 0) {
            revert AlreadyProven();
        }

        // Store validity proof data (deleted on revert)
        validChildRootClaim = childContract.rootClaim().raw();
        validChildBlobsHash = childContract.blobsHash();

        // Calculate the expected precondition hash if blob data is necessary for proposal
        bytes32 preconditionHash = bytes32(0x0);
        if (PROPOSAL_OUTPUT_COUNT > 1) {
            preconditionHash = sha256(
                abi.encodePacked(
                    uint64(l2BlockNumber()),
                    uint64(PROPOSAL_OUTPUT_COUNT),
                    uint64(OUTPUT_BLOCK_SPAN),
                    validChildBlobsHash
                )
            );
        }

        // Calculate the expected block number
        uint64 claimBlockNumber = uint64(l2BlockNumber() + PROPOSAL_OUTPUT_COUNT * OUTPUT_BLOCK_SPAN);

        // Construct the expected journal
        bytes32 journalDigest = sha256(
            abi.encodePacked(
                // The address of the recipient of the payout for this proof
                payoutRecipient,
                // The blob equivalence precondition hash
                preconditionHash,
                // The L1 head hash containing the safe L2 chain data that may reproduce the L2 head hash.
                childContract.l1Head().raw(),
                // The accepted output
                rootClaim().raw(),
                // The proposed output
                validChildRootClaim,
                // The claim block number
                claimBlockNumber,
                // The rollup configuration hash
                ROLLUP_CONFIG_HASH
            )
        );

        // Revert on proof verification failure
        RISC_ZERO_VERIFIER.verify(encodedSeal, FPVM_IMAGE_ID, journalDigest);

        // Set the game's prover address
        prover[0][0] = payoutRecipient;

        // Set the proving timestamp
        provenAt[0][0] = Timestamp.wrap(uint64(block.timestamp));
    }

    // ------------------------------
    // Fault proving
    // ------------------------------

    /// @notice Verifies that an intermediate output was part of the proposal
    function verifyIntermediateOutput(
        uint64 outputNumber,
        uint256 outputFe,
        bytes calldata blobCommitment,
        bytes calldata kzgProof
    ) external virtual returns (bool success);

    /// @notice Proves which parties computed a wrong output commitment in a tournament match
    function proveOutputFault(
        address payoutRecipient,
        uint64[3] calldata uvo,
        bytes calldata encodedSeal,
        bytes32 acceptedOutputHash,
        uint256[2] calldata proposedOutputFe,
        bytes32 computedOutputHash,
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
        if (proposedOutputFe[0] == proposedOutputFe[1]) {
            revert NoConflict();
        }

        // INVARIANT: Proofs can only pertain to computed outputs
        if (uvo[2] >= PROPOSAL_OUTPUT_COUNT) {
            revert InvalidDisputedClaimIndex();
        }

        bytes32 preconditionHash = bytes32(0x0);
        // INVARIANT: Published data is equivalent until divergent blob
        if (uvo[2] > 0) {
            // Find the divergent blob index
            uint256 divergentBlobIndex = KailuaLib.blobIndex(uvo[2]);
            // Ensure blob hashes are equal until divergence
            for (uint256 i = 0; i < divergentBlobIndex; i++) {
                if (childContracts[0].proposalBlobHashes(i).raw() != childContracts[1].proposalBlobHashes(i).raw()) {
                    revert BlobHashMismatch(
                        childContracts[0].proposalBlobHashes(i).raw(), childContracts[1].proposalBlobHashes(i).raw()
                    );
                }
            }
            // Update required precondition hash from proof if not at a boundary
            uint64 divergenceIndex = uint64(KailuaLib.fieldElementIndex(uvo[2]));
            if (divergenceIndex != 0) {
                preconditionHash = sha256(
                    abi.encodePacked(
                        divergenceIndex,
                        childContracts[0].proposalBlobHashes(divergentBlobIndex).raw(),
                        childContracts[1].proposalBlobHashes(divergentBlobIndex).raw()
                    )
                );
            }
        }

        // Validate the common output root.
        if (uvo[2] == 0) {
            // Note: acceptedOutputHash cannot be a reduced fe because the comparison below will fail
            // The safe output is the parent game's output when proving the first output
            require(acceptedOutputHash == rootClaim().raw(), "bad acceptedOutput");
        } else {
            // Note: acceptedOutputHash cannot be a reduced fe because the journal would not be provable
            uint256 acceptedOutputFe = KailuaLib.hashToFe(acceptedOutputHash);
            // Prove common output publication
            require(
                childContracts[0].verifyIntermediateOutput(
                    uvo[2] - 1, acceptedOutputFe, blobCommitments[0][0], kzgProofs[0][0]
                ),
                "bad left child acceptedOutput kzg proof"
            );
            require(
                childContracts[1].verifyIntermediateOutput(
                    uvo[2] - 1, acceptedOutputFe, blobCommitments[1][0], kzgProofs[1][0]
                ),
                "bad right child acceptedOutput kzg proof"
            );
        }

        // Validate the claimed output roots.
        if (uvo[2] == PROPOSAL_OUTPUT_COUNT - 1) {
            // Note: proposedOutputFe[] members must be canonical points or comparisons below will fail
            require(
                proposedOutputFe[0] == KailuaLib.hashToFe(childContracts[0].rootClaim().raw()), "bad proposedOutput[0]"
            );
            require(
                proposedOutputFe[1] == KailuaLib.hashToFe(childContracts[1].rootClaim().raw()), "bad proposedOutput[1]"
            );
        } else {
            // Note: proposedOutputFe[] members must be canonical points or point eval precompile calls will fail
            // Prove divergent output publication
            require(
                childContracts[0].verifyIntermediateOutput(
                    uvo[2],
                    proposedOutputFe[0],
                    blobCommitments[0][blobCommitments[0].length - 1],
                    kzgProofs[0][kzgProofs[0].length - 1]
                ),
                "bad left child proposedOutput kzg proof"
            );

            require(
                childContracts[1].verifyIntermediateOutput(
                    uvo[2],
                    proposedOutputFe[1],
                    blobCommitments[1][blobCommitments[1].length - 1],
                    kzgProofs[1][kzgProofs[1].length - 1]
                ),
                "bad right child proposedOutput kzg proof"
            );
        }

        // Construct the expected journal
        {
            uint64 claimBlockNumber = uint64(l2BlockNumber() + (uvo[2] + 1) * OUTPUT_BLOCK_SPAN);
            bytes32 journalDigest = sha256(
                abi.encodePacked(
                    // The address of the recipient of the payout for this proof
                    payoutRecipient,
                    // The blob equivalence precondition hash
                    preconditionHash,
                    // The L1 head hash containing the safe L2 chain data that may reproduce the L2 head hash.
                    childContracts[1].l1Head().raw(),
                    // The latest finalized L2 output root.
                    acceptedOutputHash,
                    // The L2 output root claim.
                    computedOutputHash,
                    // The L2 claim block number.
                    claimBlockNumber,
                    // The rollup configuration hash
                    ROLLUP_CONFIG_HASH,
                    // The FPVM Image ID
                    FPVM_IMAGE_ID
                )
            );

            // reverts on failure
            RISC_ZERO_VERIFIER.verify(encodedSeal, FPVM_IMAGE_ID, journalDigest);
        }

        resolveDispute(payoutRecipient, uvo, proposedOutputFe, KailuaLib.hashToFe(computedOutputHash));
    }

    /// @notice Proves which parties published invalid trailing data in a tournament match
    function proveTrailFault(
        address payoutRecipient,
        uint64[3] calldata uvo,
        bytes calldata encodedSeal,
        uint256[2] calldata proposedOutputFe,
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
        if (proposedOutputFe[0] == proposedOutputFe[1]) {
            revert NoConflict();
        }

        // INVARIANT: Proofs can only be accepted for equivalent proposals
        if (childContracts[0].rootClaim().raw() != childContracts[1].rootClaim().raw()) {
            revert InvalidDisputedClaimIndex();
        }

        // INVARIANT: Proofs can only pertain to trailing blob data
        if (uvo[2] < PROPOSAL_OUTPUT_COUNT) {
            revert InvalidDataRemainder();
        }

        // Because the root claim is considered the last published output, we shift the output offset down by one to
        // correctly point to the target trailing zero output
        uint64 trailOffset = uvo[2] - 1;

        // INVARIANT: The trail divergence occurs at the last blob
        if (KailuaLib.blobIndex(trailOffset) != PROPOSAL_BLOBS - 1) {
            revert InvalidDisputedClaimIndex();
        }

        // Ensure blob hashes are equal until divergence
        for (uint256 i = 0; i < PROPOSAL_BLOBS - 1; i++) {
            if (childContracts[0].proposalBlobHashes(i).raw() != childContracts[1].proposalBlobHashes(i).raw()) {
                revert BlobHashMismatch(
                    childContracts[0].proposalBlobHashes(i).raw(), childContracts[1].proposalBlobHashes(i).raw()
                );
            }
        }

        // Update required precondition hash from proof
        uint64 divergenceIndex = uint64(KailuaLib.fieldElementIndex(trailOffset));
        require(divergenceIndex > 0, "Trail data does not exist at boundaries");
        bytes32 preconditionHash = sha256(
            abi.encodePacked(
                divergenceIndex,
                childContracts[0].proposalBlobHashes(PROPOSAL_BLOBS - 1).raw(),
                childContracts[1].proposalBlobHashes(PROPOSAL_BLOBS - 1).raw()
            )
        );

        // Validate the claimed output roots publications
        {
            // Note: proposedOutputFe[] must contain canonical field elements or point eval precompile calls will fail
            require(
                childContracts[0].verifyIntermediateOutput(
                    trailOffset,
                    proposedOutputFe[0],
                    blobCommitments[0][blobCommitments[0].length - 1],
                    kzgProofs[0][kzgProofs[0].length - 1]
                ),
                "bad left child proposedOutput kzg proof"
            );

            require(
                childContracts[1].verifyIntermediateOutput(
                    trailOffset,
                    proposedOutputFe[1],
                    blobCommitments[1][blobCommitments[1].length - 1],
                    kzgProofs[1][kzgProofs[1].length - 1]
                ),
                "bad right child proposedOutput kzg proof"
            );
        }

        // Construct the expected precondition-only journal
        {
            bytes32 journalDigest = sha256(
                abi.encodePacked(
                    // The address of the recipient of the payout for this proof
                    payoutRecipient,
                    // The blob equivalence precondition hash
                    preconditionHash,
                    // The L1 head hash containing the safe L2 chain data that may reproduce the L2 head hash.
                    l1Head().raw(),
                    // The latest finalized L2 output root.
                    rootClaim().raw(),
                    // The L2 output root claim.
                    rootClaim().raw(),
                    // The L2 claim block number.
                    uint64(l2BlockNumber()),
                    // The rollup configuration hash
                    ROLLUP_CONFIG_HASH,
                    // The FPVM Image ID
                    FPVM_IMAGE_ID
                )
            );

            // reverts on failure
            RISC_ZERO_VERIFIER.verify(encodedSeal, FPVM_IMAGE_ID, journalDigest);
        }

        // Update dispute status based on trailing data
        resolveDispute(payoutRecipient, uvo, proposedOutputFe, 0x0);
    }

    function resolveDispute(
        address payoutRecipient,
        uint64[3] calldata uvo,
        uint256[2] calldata proposedOutputFe,
        uint256 expectedOutputFe
    ) internal {
        // Update proof status based on expected output
        if (proposedOutputFe[0] != expectedOutputFe) {
            // u lose
            if (proposedOutputFe[1] != expectedOutputFe) {
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

        // Announce proof status
        emit Proven(uvo[0], uvo[1], proofStatus[uvo[0]][uvo[1]]);

        // Set the game's prover address
        prover[uvo[0]][uvo[1]] = payoutRecipient;

        // Set the game's proving timestamp
        provenAt[uvo[0]][uvo[1]] = Timestamp.wrap(uint64(block.timestamp));
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
