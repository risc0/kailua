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

    /// @notice The duration after which the proposal is accepted
    Duration internal immutable MAX_CLOCK_DURATION;

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

    /// @notice Denotes which proposers have been challenged by whom
    mapping(address => mapping(address => bool)) public proposerChallengerMatrix;

    /// @notice The index of the last challenge against an output
    mapping(uint64 => mapping(uint64 => uint64)) public outputLastChallengeIndex;
    /// @notice The provability of an output of a proposal
    mapping(uint64 => mapping(uint64 => ProofStatus)) public outputProofStatus;

    constructor(
        IDisputeGameFactory _disputeGameFactory,
        GameType _gameType,
        uint256 _proposerBond,
        uint256 _challengerBond,
        Duration _maxClockDuration
    ) {
        // Instantiate constants
        DISPUTE_GAME_FACTORY = _disputeGameFactory;
        GAME_TYPE = _gameType;
        PROPOSER_BOND = _proposerBond;
        CHALLENGER_BOND = _challengerBond;
        MAX_CLOCK_DURATION = _maxClockDuration;
        // Reserve index 0 moves
        proposals.push(
            ProposalData({
                proposerAddress: address(0x0),
                gameContract: IFaultAttributionGame(address(0x0)),
                previousProposalIndex: 0,
                challengeCount: 0
            })
        );
        challenges.push(
            ChallengeData({
                challengerAddress: address(0x0),
                proposalIndex: 0,
                outputOffset: 0,
                previousChallengeIndex: 0,
                challengeBelowIndex: 0,
                challengeAboveIndex: 0
            })
        );
    }

    // ------------------------------
    // IFaultAttributionManager implementation
    // ------------------------------
    // todo: reorg protection + sanity checks

    function gameAtIndex(uint64 index) public view returns (IFaultAttributionGame) {
        return proposals[index].gameContract;
    }

    /// @notice Creates a new dispute game for a proposer using the DisputeGameFactory
    function propose(Claim claimedOutputRoot, uint64 l2BlockNumber, uint64 parentGameIndex, uint64 intermediateOutputs)
        external
        payable
        onlyBondedProposer
    {
        // Require parent game to not have been invalidated
        IFaultAttributionGame parentGameContract = proposals[parentGameIndex].gameContract;
        if (parentGameContract.status() == GameStatus.CHALLENGER_WINS) {
            revert InvalidParent();
        }
        // Invoke the dispute game factory to create a new game instance
        bytes memory extraData = abi.encodePacked(l2BlockNumber, parentGameIndex, intermediateOutputs);
        IFaultAttributionGame gameContract =
            IFaultAttributionGame(address(DISPUTE_GAME_FACTORY.create(GAME_TYPE, claimedOutputRoot, extraData)));
        // Record the proposer's move
        proposals.push(
            ProposalData({
                proposerAddress: msg.sender,
                gameContract: gameContract,
                previousProposalIndex: proposerPreviousIndex[msg.sender],
                challengeCount: 0
            })
        );
        proposerPreviousIndex[msg.sender] = uint64(proposals.length - 1);
    }

    function challenge(uint64 proposalIndex, uint64 outputOffset, uint64 challengeBelowIndex)
        external
        payable
        onlyBondedChallenger
    {
        require(proposalIndex > 0);
        IFaultAttributionGame gameContract = proposals[proposalIndex].gameContract;
        // INVARIANT: Challenges cannot be created unless the game is currently in progress.
        if (gameContract.status() != GameStatus.IN_PROGRESS) {
            revert GameNotInProgress();
        }
        // INVARIANT: No prior challenge against same proposer by challenger
        if (proposerChallengerMatrix[proposals[proposalIndex].proposerAddress][msg.sender]) {
            revert AlreadyChallenged();
        }
        // INVARIANT: The challenge targets a proposed output.
        if (gameContract.intermediateOutputs() < outputOffset) {
            revert NotProposed();
        }
        // INVARIANT: The challenge clock has not expired
        if (gameContract.getChallengerDuration().raw() >= MAX_CLOCK_DURATION.raw()) {
            revert ClockExpired();
        }
        // INVARIANT: The challenge targets a possibly faulty output
        if (outputProofStatus[proposalIndex][outputOffset] == ProofStatus.INTEGRITY) {
            revert AlreadyProven();
        }

        // INVARIANT: This challenge has the correct priority
        if (outputLastChallengeIndex[proposalIndex][outputOffset] != challengeBelowIndex) {
            revert AlreadyChallenged();
        }
        // Record the challenger's move
        uint64 challengeIndex = uint64(challenges.length);
        if (challengeBelowIndex == 0) {
            // Record possible increment of number of challenged outputs
            proposals[proposalIndex].challengeCount++;
        } else {
            // Update previous challenge against this output to point upwards to this new challenge
            challenges[challengeBelowIndex].challengeAboveIndex = challengeIndex;
        }
        // Insert challenge into challenger timeline
        challenges.push(
            ChallengeData({
                challengerAddress: msg.sender,
                proposalIndex: proposalIndex,
                outputOffset: outputOffset,
                previousChallengeIndex: challengerPreviousIndex[msg.sender],
                challengeBelowIndex: challengeBelowIndex,
                challengeAboveIndex: 0
            })
        );
        // Update the latest challenge against this output
        outputLastChallengeIndex[proposalIndex][outputOffset] = challengeIndex;
        // Update this challenger's latest move
        challengerPreviousIndex[msg.sender] = challengeIndex;
        // Update the proposer-challenger matrix
        proposerChallengerMatrix[proposals[proposalIndex].proposerAddress][msg.sender] = true;
    }

    function prove(
        uint64 challengeIndex,
        bytes calldata encodedSeal,
        bytes32 acceptedOutput,
        bytes32 proposedOutput,
        bytes32 computedOutput,
        bytes[] calldata kzgProofs
    ) external payable {
        require(challengeIndex > 0);
        ChallengeData memory challenge = challenges[challengeIndex];
        IFaultAttributionGame gameContract = proposals[challenge.proposalIndex].gameContract;
        // INVARIANT: Proofs cannot be submitted unless the game is currently in progress.
        if (gameContract.status() != GameStatus.IN_PROGRESS) {
            revert GameNotInProgress();
        }
        // INVARIANT: Proofs can only be submitted once
        if (outputProofStatus[challenge.proposalIndex][challenge.outputOffset] != ProofStatus.NONE) {
            revert AlreadyProven();
        }

        // Forward proof to dispute contract to validate
        ProofStatus proofStatus = gameContract.prove(
            challenge.outputOffset, encodedSeal, acceptedOutput, proposedOutput, computedOutput, kzgProofs
        );
        // Record new proof status
        outputProofStatus[challenge.proposalIndex][challenge.outputOffset] = proofStatus;
        // Decrement unique challenge counter if output proven true
        if (proofStatus == ProofStatus.INTEGRITY) {
            proposals[challenge.proposalIndex].challengeCount--;
            // todo: inherit the challenger / reject the challenge
        }
    }

    /// @notice Finalizes an honest unchallenged proposal
    function acceptProposal(uint64 proposalIndex) external {
        require(proposalIndex > 0);
        IFaultAttributionGame gameContract = proposals[proposalIndex].gameContract;
        // INVARIANT: Resolution cannot occur unless the game is currently in progress.
        if (gameContract.status() != GameStatus.IN_PROGRESS) {
            revert GameNotInProgress();
        }
        // INVARIANT: Resolution cannot occur unless the parent proposal is accepted.
        if (gameContract.parentGame().status() != GameStatus.DEFENDER_WINS) {
            revert OutOfOrderResolution();
        }
        // INVARIANT: Cannot resolve proposal while there are unresolved challenges.
        if (proposals[proposalIndex].challengeCount > 0) {
            revert OutOfOrderResolution();
        }
        // INVARIANT: Require proposer's prior proposal(s) to have been accepted.
        uint64 previousProposalIndex = proposals[proposalIndex].previousProposalIndex;
        if (previousProposalIndex > 0) {
            IFaultAttributionGame previousGameContract = proposals[previousProposalIndex].gameContract;
            if (previousGameContract.status() != GameStatus.DEFENDER_WINS) {
                revert OutOfOrderResolution();
            }
        }
        // INVARIANT: Cannot resolve unless the challenge clock has expired
        if (gameContract.getChallengerDuration().raw() < MAX_CLOCK_DURATION.raw()) {
            revert ClockNotExpired();
        }

        // Finalize through game contract
        gameContract.resolve();
    }

    /// @notice Rejects a proposal and its proposer's subsequent proposals
    function rejectProposal(uint64 challengeIndex) external {
        require(challengeIndex > 0);
        ChallengeData memory challenge = challenges[challengeIndex];
        IFaultAttributionGame gameContract = proposals[challenge.proposalIndex].gameContract;
        // INVARIANT: The proposal's game is currently in progress.
        if (gameContract.status() != GameStatus.IN_PROGRESS) {
            revert UnchallengedGame();
        }
        // INVARIANT: The proposal's parent game is finalized.
        if (gameContract.parentGame().status() != GameStatus.DEFENDER_WINS) {
            revert OutOfOrderResolution();
        }
        // INVARIANT: Proposer's prior proposal(s) have been resolved without fault
        uint64 previousProposalIndex = proposals[challenge.proposalIndex].previousProposalIndex;
        if (previousProposalIndex > 0) {
            IFaultAttributionGame previousGameContract = proposals[previousProposalIndex].gameContract;
            if (previousGameContract.status() != GameStatus.DEFENDER_WINS) {
                revert OutOfOrderResolution();
            }
        }
        // INVARIANT: Fault has been proven
        if (outputProofStatus[challenge.proposalIndex][challenge.outputOffset] != ProofStatus.FAULT) {
            revert NotProven();
        }
        // INVARIANT: The challenge clock has expired
        if (gameContract.getChallengerDuration().raw() < MAX_CLOCK_DURATION.raw()) {
            revert ClockNotExpired();
        }
        // INVARIANT: The challenger has no prior unclosed challenges
        if (challenge.previousChallengeIndex > 0) {
            uint64 lastChallengeProposalIndex = challenges[challenge.previousChallengeIndex].proposalIndex;
            IFaultAttributionGame lastChallengeGameContract = proposals[lastChallengeProposalIndex].gameContract;
            if (lastChallengeGameContract.status() != GameStatus.CHALLENGER_WINS) {
                revert OutOfOrderResolution();
            }
        }
        // INVARIANT: This is the canonical challenge type
        if (proposals[challenge.proposalIndex].challengeCount != 1) {
            revert OutOfOrderResolution();
        }
        // INVARIANT: The challenger had priority
        if (challenge.challengeBelowIndex != 0) {
            revert OutOfOrderResolution();
        }
        // todo blacklist proposer
        // todo pay bond to original canonical challenger
        // todo reject this proposal
        // todo reject all subsequent proposals by proposer?
    }

    /// @notice Revokes a challenge because it was not the canonical challenge against this proposer
    function revokeFaultChallenge(uint64 canonicalIndex, uint64 revokedIndex) external payable onlyBondedChallenger {
        require(canonicalIndex > 0);
        ChallengeData memory canonicalChallenge = challenges[canonicalIndex];
        require(revokedIndex > 0);
        ChallengeData memory revokedChallenge = challenges[revokedIndex];
        // INVARIANT: todo The challenges are against the same proposer
        //        if (canonicalChallenge.challengerAddress == revokedChallenge.challengerAddress) {
        //            revert NoCreditToClaim();
        //        }
        // INVARIANT: The revoked challenge does not prove integrity
        if (outputProofStatus[revokedChallenge.proposalIndex][revokedChallenge.outputOffset] == ProofStatus.INTEGRITY) {
            revert AlreadyProven();
        }
        // INVARIANT: The canonical challenge proves fault
        if (outputProofStatus[canonicalChallenge.proposalIndex][canonicalChallenge.outputOffset] != ProofStatus.FAULT) {
            revert NotProven();
        }
        // INVARIANT: todo The preceding challenge was canonical against the proposer

        if (canonicalChallenge.proposalIndex == revokedChallenge.proposalIndex) {
            if (canonicalChallenge.outputOffset == revokedChallenge.outputOffset) {
                // Revoking redundant challenges with lower priority
//                require(canonicalChallenge.challengePriority < revokedChallenge.challengePriority);
            } else {
                // Revoking challenges that target a higher offset
                require(canonicalChallenge.outputOffset < revokedChallenge.outputOffset);
            }
        } else {
            // Revoking challenges that target another proposal
            require(canonicalChallenge.proposalIndex < revokedChallenge.proposalIndex);
        }

        // todo: inherit all preceding challenges
        // todo: remove all subsequent challenges
    }

    /// @notice Replaces the owner of all unresolved challenges by the faulty challenger with msg.sender
    ///         until challengeIndex, and deletes all subsequent challenges by the faulty challenger.
    function replaceFaultyChallenger(uint64 firstFaultIndex, uint64 subsequentOwnedIndex) internal {
        require(firstFaultIndex > 0);
        // INVARIANT: No self-inheritance
        address faultyChallenger = challenges[firstFaultIndex].challengerAddress;
        if (faultyChallenger == msg.sender) {
            revert BadAuth();
        }
        // Start from the last challenge done by this
        uint64 challengeIndex = challengerPreviousIndex[faultyChallenger];
        // Delete all moves starting from the first faulty one
        while (challengeIndex >= firstFaultIndex) {
            ChallengeData storage faultyChallenge = challenges[challengeIndex];
            if (faultyChallenge.challengeBelowIndex == 0 && faultyChallenge.challengeAboveIndex == 0) {
                if (
                    outputProofStatus[faultyChallenge.proposalIndex][faultyChallenge.outputOffset]
                        != ProofStatus.INTEGRITY
                ) {
                    // This is the last challenge against this output, reduce the unique challenge counter
                    proposals[faultyChallenge.proposalIndex].challengeCount--;
                }
            } else {
                // More challenges remain, collapse the doubly-linked list
                if (faultyChallenge.challengeAboveIndex > 0) {
                    // Point the challenge above to the challenge below this one
                    challenges[faultyChallenge.challengeAboveIndex].challengeBelowIndex =
                        faultyChallenge.challengeBelowIndex;
                }
                if (faultyChallenge.challengeBelowIndex > 0) {
                    // Point the challenge below to the challenge above this one
                    challenges[faultyChallenge.challengeBelowIndex].challengeAboveIndex =
                        faultyChallenge.challengeAboveIndex;
                }
            }
            if (outputLastChallengeIndex[faultyChallenge.proposalIndex][faultyChallenge.outputOffset] == challengeIndex)
            {
                // Point the last challenge index to the challenge below this one
                outputLastChallengeIndex[faultyChallenge.proposalIndex][faultyChallenge.outputOffset] =
                    faultyChallenge.challengeBelowIndex;
            }
            // move to challenger's previous move in the timeline
            uint64 nextChallengeIndex = faultyChallenge.previousChallengeIndex;
            // clear storage
            delete challenges[challengeIndex];
            challengeIndex = nextChallengeIndex;
        }
        // Inherit all moves prior to the first faulty one
        ChallengeData storage subsequentChallenge = challenges[subsequentOwnedIndex];
        // INVARIANT: The pointer targets a challenge owned by the inheritor
        if (subsequentChallenge.challengerAddress != msg.sender) {
            revert BadAuth();
        }
        // INVARIANT: No future move qualifies as a better starting pointer
        if (subsequentOwnedIndex < challengeIndex && challengerPreviousIndex[msg.sender] > subsequentOwnedIndex) {
            revert BadExtraData();
        }
        while (challengeIndex > 0) {
            ChallengeData storage inheritedChallenge = challenges[challengeIndex];
            // INVARIANT: The challenge was not resolved
            IFaultAttributionGame proposalContract = proposals[inheritedChallenge.proposalIndex].gameContract;
            if (proposalContract.status() == GameStatus.CHALLENGER_WINS) {
                break;
            }
            // INVARIANT: The challenge is not redundant
            if (proposerChallengerMatrix[proposals[inheritedChallenge.proposalIndex].proposerAddress][msg.sender]) {
                revert AlreadyChallenged();
            }
            // Move inheritor's pointer back while necessary
            while (subsequentOwnedIndex > 0 && challenges[subsequentOwnedIndex].previousChallengeIndex > challengeIndex)
            {
                subsequentOwnedIndex = challenges[subsequentOwnedIndex].previousChallengeIndex;
            }
            // INVARIANT: The challenge is not being placed behind a resolved challenge??
            // todo
            // Inherit challenge
            inheritedChallenge.challengerAddress = msg.sender;
            if (subsequentOwnedIndex > challengeIndex) {}
            else if (subsequentOwnedIndex > 0) {}
            else {}
        }
    }

    function resolveChallenge(uint64 proposalIndex, uint64 outputOffset) external {
        require(proposalIndex > 0);
        // todo: take into consideration canonical challenge priority

        // todo: if challenge proven faulty, slash & inherit the challenger

        // todo: if proposal proven faulty, slash the proposer & terminate all following proposals

        // todo: a resolution in favor of the challenger cannot happen until all prior proposals
        // todo: to this one are resolved in favor of the proposer, o/w evidence of challenger fault
    }

    // ------------------------------
    // Utility methods
    // ------------------------------

    modifier onlyBondedProposer() {
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

        _;
    }

    modifier onlyBondedChallenger() {
        // INVARIANT: The challenger is not blacklisted
        if (challengersBlacklisted[msg.sender]) {
            revert BadAuth();
        }
        // supplement challenger's collateral with transferred value
        if (msg.value > 0) {
            challengerBonds[msg.sender] += msg.value;
        }
        // INVARIANT: The challenger staked enough collateral
        if (challengerBonds[msg.sender] != CHALLENGER_BOND) {
            revert IncorrectBondAmount();
        }

        _;
    }
}
