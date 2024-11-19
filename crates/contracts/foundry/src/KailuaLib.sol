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

/// @notice Denotes the proven status of the game
/// @custom:value NONE indicates that no proof has been submitted yet.
/// @custom:value FAULT indicates that a valid fault proof has been submitted.
/// @custom:value INTEGRITY indicates that a valid integrity proof has been submitted.
enum ProofStatus {
    NONE,
    FAULT,
    INTEGRITY
}

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
/// @notice Thrown when a challenge is submitted against an already challenged game
error AlreadyChallenged();

// 0xf2a87d5e
/// @notice Thrown when a challenge is submitted against an out of range output
error NotProposed();

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

/// @notice Occurs when the anchored game is not finalized
error InvalidAnchoredGame();

/// @notice Occurs when the duplication counter is wrong
error InvalidDuplicationCounter();

/// @notice Occurs when the anchored game block number is different
/// @param anchored The L2 block number of the anchored game
/// @param initialized This game's l2 block number
error BlockNumberMismatch(uint256 anchored, uint256 initialized);

/// @notice Emitted when an output is challenged.
/// @param outputIndex The index of the challenged output
/// @param challenger The address of the challenge issuer
event Challenged(uint32 indexed outputIndex, address indexed challenger);

/// @notice Emitted when an output is proven.
/// @param outputIndex The index of the challenged output
/// @param status The proven status of the output
event Proven(uint32 indexed outputIndex, ProofStatus indexed status);

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

    function versionedKZGHash(bytes calldata blobCommitment) internal pure returns (bytes32 hash) {
        require(blobCommitment.length == 48);
        hash = ((sha256(blobCommitment) << 8) >> 8) | KZG_COMMITMENT_VERSION;
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
            ((value << 2) >> 2),
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

    /// @notice Transfers ETH from the contract's balance to the recipient
    function pay(uint256 amount, address recipient) internal {
        (bool success,) = recipient.call{value: amount}(hex"");
        if (!success) revert BondTransferFailed();
    }
}