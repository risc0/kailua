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

/// @notice Denotes the proven status of the game
/// @custom:value NONE indicates that no proof has been submitted yet.
enum ProofStatus {
    NONE,
    U_LOSE_V_LOSE,
    U_LOSE_V_WIN,
    U_WIN_V_LOSE
}

/// @notice Thrown when a blacklisted address attempts to interact with the game.
error Blacklisted(address source, address expected);

// 0x9d3e7d24
/// @notice Thrown when a child from an unknown source appends itself to a tournament
error UnknownGame();

// 0xc105260a
/// @notice Thrown when pruning children of an unresolved parent
error GameNotResolved();

// 0x8b1dfa22
/// @notice Thrown when eliminating an already removed child
error AlreadyEliminated();

// 0x2c06a364
/// @notice Thrown when a proof is submitted for an already proven game
error AlreadyProven();

// 0xa37b6ee4
/// @notice Thrown when a challenge is submitted after the clock has expired
error ClockExpired();

// 0xa506d334
/// @notice Thrown when a resolution is attempted for an unproven claim
error NotProven();

// 0x87ec6473
/// @notice Thrown when a proving fault for an unchallenged game
error UnchallengedGame();

// 0x84b45441
/// @notice Thrown when a proving fault for an unchallenged output
error UnchallengedOutput();

// 0xf1082a93
/// @notice Thrown when resolving a faulty proposal
error ProvenFaulty();

// 0xf2a87d5e
/// @notice Thrown when pruning is attempted with no children
error NotProposed();

// 0x7412124e
/// @notice Thrown when proving is attempted with two agreeing outputs
error NoConflict();

// 0x1ebb374b
/// @notice Thrown when a game is created with a parent instance from another game type
error GameTypeMismatch(GameType parentType, GameType expectedType);

// 0xe5f91edd
/// @notice Thrown when a game is initialized for more blocks than the maximum allowed
error BlockCountExceeded(uint256 l2BlockNumber, uint256 rootBlockNumber);

// 0x1844c87b
/// @notice Thrown when an incorrect blob hash is provided
error BlobHashMismatch(bytes32 found, bytes32 expected);

// 0x6dafbcfa
/// @notice Thrown when a blob hash is missing
error BlobHashMissing(uint256 index, uint256 count);

// 0x1be40a37
/// @notice Occurs when the anchored game is not finalized
error InvalidAnchoredGame();

// 0x19e3a1dc
/// @notice Occurs when the duplication counter is wrong
error InvalidDuplicationCounter();

// 0xeaa0996e
/// @notice Occurs when the anchored game block number is different
/// @param anchored The L2 block number of the anchored game
/// @param initialized This game's l2 block number
error BlockNumberMismatch(uint256 anchored, uint256 initialized);

/// @notice Emitted when an output is proven.
/// @param u The preexisting proposal
/// @param v The subsequent proposal
/// @param status The proven status of the match
event Proven(uint64 indexed u, uint64 indexed v, ProofStatus indexed status);

/// @notice Emitted when the participation bond is updated
/// @param amount The new required bond amount
event BondUpdated(uint256 amount);

interface IKailuaTreasury {
    /// @notice Returns the game index at which proposer was proven faulty
    function eliminationRound(address proposer) external returns (uint256);

    /// @notice Returns the proposer of a game
    function proposerOf(address game) external returns (address);

    /// @notice Eliminates a child's proposer and transfers their bond to the prover
    function eliminate(address child, address prover) external;

    /// @notice Returns true iff a proposal is currently being submitted
    function isProposing() external returns (bool);
}

library KailuaLib {
    /// @notice The KZG commitment version
    bytes32 internal constant KZG_COMMITMENT_VERSION =
        bytes32(0x0100000000000000000000000000000000000000000000000000000000000000);

    /// @notice The modular exponentiation precompile
    address internal constant MOD_EXP = address(0x05);

    /// @notice The point evaluation precompile
    address internal constant KZG = address(0x0a);

    /// @notice Scalar field modulus of BLS12-381
    uint256 internal constant BLS_MODULUS =
        52435875175126190479447740508185965837690552500527637822603658699938581184513;

    /// @notice The base root of unity for indexing blob field elements
    uint256 internal constant ROOT_OF_UNITY =
        39033254847818212395286706435128746857159659164139250548781411570340225835782;

    /// @notice The po2 for the number of field elements in a single blob
    uint256 internal constant FIELD_ELEMENTS_PER_BLOB_PO2 = 12;

    function blobIndex(uint256 element) internal pure returns (uint256 index) {
        index = element / (1 << FIELD_ELEMENTS_PER_BLOB_PO2);
    }

    function blobPosition(uint256 element) internal pure returns (uint256 position) {
        position = element % (1 << FIELD_ELEMENTS_PER_BLOB_PO2);
    }

    function versionedKZGHash(bytes calldata blobCommitment) internal pure returns (bytes32 hash) {
        require(blobCommitment.length == 48);
        hash = ((sha256(blobCommitment) << 8) >> 8) | KZG_COMMITMENT_VERSION;
    }

    function hashToFe(bytes32 hash) internal pure returns (bytes32 fe) {
        fe = ((hash << 2) >> 2);
    }

    function verifyKZGBlobProof(
        bytes32 versionedHash,
        uint32 index,
        bytes32 value,
        bytes calldata blobCommitment,
        bytes calldata proof
    ) internal returns (bool success) {
        uint256 rootOfUnity = modExp(reverseBits(index));

        bytes memory kzgCallData = abi.encodePacked(
            versionedHash, // proposalBlobHash().raw(),
            rootOfUnity,
            hashToFe(value),
            blobCommitment,
            proof
        );
        (success,) = KZG.call(kzgCallData);
    }

    function modExp(uint256 base) internal returns (uint256 result) {
        bytes memory modExpData =
            abi.encodePacked(uint256(32), uint256(32), uint256(32), ROOT_OF_UNITY, base, BLS_MODULUS);
        (, bytes memory rootOfUnity) = MOD_EXP.call(modExpData);
        result = uint256(bytes32(rootOfUnity));
    }

    function reverseBits(uint32 index) internal pure returns (uint256 result) {
        for (uint256 i = 0; i < FIELD_ELEMENTS_PER_BLOB_PO2; i++) {
            result <<= 1;
            result |= ((1 << i) & index) >> i;
        }
    }
}
