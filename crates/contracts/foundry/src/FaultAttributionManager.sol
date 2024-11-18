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

    /// @notice Stores all proposers and their moves
    Proposer[] internal proposers;
    /// @notice Stores all challengers and their moves
    Challenger[] internal challengers;

    /// @notice The index of the last challenge against an output
    mapping(address => mapping(uint64 => uint64[2])) public outputChallengeTip;
    /// @notice The provability of an output of a proposal
    mapping(address => mapping(uint64 => ProofStatus)) public outputProofStatus;

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
    }

    // ------------------------------
    // IFaultAttributionManager implementation
    // ------------------------------
    // todo: reorg protection + sanity checks

    function gameAtProposerIndex(uint64 proposerIndex, uint64 proposalIndex) public view returns (IFaultAttributionGame) {
        return proposers[proposerIndex].proposals[proposalIndex].gameContract;
    }

    /// @notice Creates a new dispute game for a proposer using the DisputeGameFactory
    function propose(
        uint64 proposerIndex,
        Claim claimedOutputRoot,
        uint64 l2BlockNumber,
        uint64 parentGameProposer,
        uint64 parentGameIndex,
        uint64 intermediateOutputs
    )
        external
        payable
        onlyBondedProposer(proposerIndex)
    {
        // Require parent game to not have been invalidated
        IFaultAttributionGame parentGameContract = gameAtProposerIndex(parentGameProposer, parentGameIndex);
        if (parentGameContract.status() == GameStatus.CHALLENGER_WINS) {
            revert InvalidParent();
        }
        // Invoke the dispute game factory to create a new game instance
        bytes memory extraData = abi.encodePacked(l2BlockNumber, parentGameProposer, parentGameProposer, intermediateOutputs);
        IFaultAttributionGame gameContract =
            IFaultAttributionGame(address(DISPUTE_GAME_FACTORY.create(GAME_TYPE, claimedOutputRoot, extraData)));
        // Record the proposer's move
        proposers[proposerIndex].proposals.push(
            Proposal({
                gameContract: gameContract,
                challengeCount: 0
            })
        );
    }

    function challenge(
        uint64 challengerIndex,
        uint64 proposerIndex,
        uint64 proposalIndex,
        uint64 outputOffset,
        uint64[2] previousChallenge
    )
        external
        payable
        onlyBondedChallenger(challengerIndex)
    {
        IFaultAttributionGame gameContract = gameAtProposerIndex(proposerIndex, proposalIndex);
        // INVARIANT: No prior challenge against same proposer by challenger
        if (challengers[challengerIndex].challengedProposers[proposerIndex]) {
            revert AlreadyChallenged();
        }
        // INVARIANT: Challenges cannot be created unless the game is currently in progress.
        if (gameContract.status() != GameStatus.IN_PROGRESS) {
            revert GameNotInProgress();
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
        if (outputProofStatus[gameContract][outputOffset] == ProofStatus.INTEGRITY) {
            revert AlreadyProven();
        }
        // INVARIANT: This challenge has the correct priority
        if (outputChallengeTip[gameContract][outputOffset] != previousChallenge) {
            revert AlreadyChallenged();
        }
        // Record the challenger's move
        uint64 newChallengeTip = [challengerIndex, challengers[challengerIndex].challenges.length];
        outputChallengeTip[gameContract][outputOffset] = newChallengeTip;
        if (previousChallenge[0] == previousChallenge[1] == 0) {
            // Record possible increment of number of challenged outputs
            proposers[proposerIndex].proposals[proposalIndex].challengeCount++;
        } else {
            challengers[previousChallenge[0]].challenges[previousChallenge[1]].nextChallenge = newChallengeTip;
        }
        // Insert challenge into challenger timeline
        challengers[challengerIndex].challenges.push(Challenge({
            proposerIndex: proposerIndex,
            proposalIndex: proposalIndex,
            outputOffset: outputOffset,
            previousChallenge: previousChallenge,
            nextChallenge: [0, 0]
        }));
        // Update the proposer-challenger matrix
        challengers[challengerIndex].challengedProposers[proposerIndex] = true;
    }

    function prove(
        uint64 challengerIndex,
        uint64 challengeIndex,
        bytes calldata encodedSeal,
        bytes32 acceptedOutput,
        bytes32 proposedOutput,
        bytes32 computedOutput,
        bytes[] calldata kzgProofs
    ) external payable {
        Challenge storage challenge = challengers[challengerIndex].challenges[challengeIndex];
        IFaultAttributionGame gameContract = gameAtProposerIndex(challenge.proposerIndex, challenge.proposalIndex);
        // INVARIANT: Proofs cannot be submitted unless the game is currently in progress.
        if (gameContract.status() != GameStatus.IN_PROGRESS) {
            revert GameNotInProgress();
        }
        // INVARIANT: Proofs can only be submitted once
        if (outputProofStatus[gameContract][challenge.outputOffset] != ProofStatus.NONE) {
            revert AlreadyProven();
        }

        // Forward proof to dispute contract to validate
        ProofStatus proofStatus = gameContract.prove(
            challenge.outputOffset, encodedSeal, acceptedOutput, proposedOutput, computedOutput, kzgProofs
        );
        // Record new proof status
        outputProofStatus[gameContract][challenge.outputOffset] = proofStatus;
        // Decrement unique challenge counter if output proven true
        if (proofStatus == ProofStatus.INTEGRITY) {
            proposers[challenge.proposerIndex].proposals[challenge.proposalIndex].challengeCount--;
            // todo: inherit the challenger / reject the challenge
        }
    }

    /// @notice Finalizes an honest unchallenged proposal
    function acceptProposal(
        uint64 proposerIndex,
        uint64 proposalIndex
    ) external {
        IFaultAttributionGame gameContract = gameAtProposerIndex(proposerIndex, proposalIndex);
        Proposal storage proposal = proposers[proposerIndex].proposals[proposalIndex];
        // INVARIANT: Resolution cannot occur unless the game is currently in progress.
        if (gameContract.status() != GameStatus.IN_PROGRESS) {
            revert GameNotInProgress();
        }
        // INVARIANT: Resolution cannot occur unless the parent proposal is accepted.
        if (gameContract.parentGame().status() != GameStatus.DEFENDER_WINS) {
            revert OutOfOrderResolution();
        }
        // INVARIANT: Cannot resolve proposal while there are unresolved challenges.
        if (proposal.challengeCount > 0) {
            revert OutOfOrderResolution();
        }
        // INVARIANT: Require proposer's prior proposal(s) to have been accepted.
        if (proposalIndex > 0) {
            IFaultAttributionGame previousGameContract = gameAtProposerIndex(proposerIndex, proposalIndex - 1);
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
    function rejectProposal(uint64 challengerIndex, uint64 challengeIndex) external {
        Challenge storage challenge = challengers[challengerIndex].challenges[challengeIndex];
        Proposer storage proposer = proposers[challenge.proposerIndex];
        IFaultAttributionGame gameContract = gameAtProposerIndex(challenge.proposerIndex, challenge.proposalIndex);
        // INVARIANT: The proposal's game is currently in progress.
        if (gameContract.status() != GameStatus.IN_PROGRESS) {
            revert UnchallengedGame();
        }
        // INVARIANT: The proposal's parent game is finalized.
        if (gameContract.parentGame().status() != GameStatus.DEFENDER_WINS) {
            revert OutOfOrderResolution();
        }
        // INVARIANT: Proposer's prior proposal(s) have been resolved without fault
        if (challenge.proposalIndex > 0) {
            IFaultAttributionGame previousGameContract = gameAtProposerIndex(challenge.proposerIndex, challenge.proposalIndex - 1);
            if (previousGameContract.status() != GameStatus.DEFENDER_WINS) {
                revert OutOfOrderResolution();
            }
        }
        // INVARIANT: Fault has been proven
        if (outputProofStatus[gameContract][challenge.outputOffset] != ProofStatus.FAULT) {
            revert NotProven();
        }
        // INVARIANT: The challenge clock has expired
        if (gameContract.getChallengerDuration().raw() < MAX_CLOCK_DURATION.raw()) {
            revert ClockNotExpired();
        }
        // INVARIANT: The challenger has no prior unclosed challenges
        if (challengeIndex > 0) {
            Challenge storage lastChallenge = challengers[challengerIndex].challenges[challengeIndex - 1];
            IFaultAttributionGame lastChallengeGameContract = gameAtProposerIndex(lastChallenge.proposerIndex, lastChallenge.proposalIndex);
            if (lastChallengeGameContract.status() != GameStatus.CHALLENGER_WINS) {
                revert OutOfOrderResolution();
            }
        }
        // INVARIANT: This is the canonical challenge type
        if (proposer.proposals[challenge.proposalIndex].challengeCount != 1) {
            revert OutOfOrderResolution();
        }
        // INVARIANT: The challenger had priority
        if (challenge.previousChallenge[0] != 0 || challenge.previousChallenge[1] != 0) {
            revert OutOfOrderResolution();
        }
        // INVARIANT: The proposer holds the bond
        // Empty proposer's wallet
        proposer.bond = 0;
//        ProofLib.pay(challenger)
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
            if (outputChallengeCount[faultyChallenge.proposalIndex][faultyChallenge.outputOffset] == challengeIndex)
            {
                // Point the last challenge index to the challenge below this one
                outputChallengeCount[faultyChallenge.proposalIndex][faultyChallenge.outputOffset] =
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

    modifier onlyBondedProposer(uint64 proposer) {
        // only the proposer's account may proceed
        if (msg.sender != proposers[proposer].account) {
            revert BadAuth();
        }
        // supplement proposer's collateral with transferred value
        if (msg.value > 0) {
            proposers[proposer].bond += msg.value;
        }
        // ensure that the proposer has the collateral staked to make this move
        if (proposers[proposer].bond != PROPOSER_BOND) {
            revert IncorrectBondAmount();
        }

        _;
    }

    modifier onlyBondedChallenger(uint64 challenger) {
        // INVARIANT: The challenger is not blacklisted
        if (msg.sender != challengers[challenger].account) {
            revert BadAuth();
        }
        // supplement challenger's collateral with transferred value
        if (msg.value > 0) {
            challengers[challenger].bond += msg.value;
        }
        // INVARIANT: The challenger staked enough collateral
        if (challengers[challenger].bond != CHALLENGER_BOND) {
            revert IncorrectBondAmount();
        }

        _;
    }
}
