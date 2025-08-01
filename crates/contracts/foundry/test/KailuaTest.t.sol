// Copyright 2025 RISC Zero, Inc.
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

import {Test} from "forge-std/Test.sol";
import {console2} from "forge-std/console2.sol";

import "../src/vendor/FlatOPImportV1.4.0.sol";
import "../src/vendor/FlatR0ImportV2.0.2.sol";

import "../src/KailuaLib.sol";
import "../src/KailuaTournament.sol";
import "../src/KailuaTreasury.sol";
import "../src/KailuaGame.sol";

contract KailuaTest is Test {
    /// @dev Allows for the creation of clone proxies with immutable arguments.
    using LibClone for address;

    DisputeGameFactory factory;
    OptimismPortal2 portal;
    RiscZeroMockVerifier verifier;

    uint256 public constant BLOB_NZ_VALUE = 0x73eda753299d7d483339d80809a1d80553bda402fffe5bfeffffffff00000000;
    bytes public constant BLOB_NZ_COMMIT = abi.encodePacked(
        hex"b7f1d3a73197d7942695638c4fa9ac0fc3688c4f9774b905a14e3a3f171bac586c55e83ff97a1aeffb3af00adb22c6bb"
    );
    bytes public constant BLOB_ID_ELEM = abi.encodePacked(
        hex"c00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000"
    );

    function setUp() public virtual {
        // OP Stack
        factory = DisputeGameFactory(address(new DisputeGameFactory()).clone());
        factory.initialize(address(this));
        portal = OptimismPortal2(payable(address(new OptimismPortal2(0, 0)).clone()));
        portal.initialize(
            factory, SystemConfig(address(0x0)), SuperchainConfig(address(0x0)), GameType.wrap(uint32(1337))
        );
        vm.assertEq(address(portal.disputeGameFactory()), address(factory));
        // RISC Zero
        verifier = new RiscZeroMockVerifier(bytes4(bytes32(uint256(0xFF))));
    }

    function deployKailua(
        uint64 proposalOutputCount,
        uint64 outputBlockSpan,
        bytes32 rootClaim,
        uint64 l2BlockNumber,
        uint256 genesisTimestamp,
        uint256 l2BlockTime,
        uint64 maxClockDuration
    ) public returns (KailuaTreasury treasury, KailuaGame game, KailuaTournament anchor) {
        // Kailua
        treasury = new KailuaTreasury(
            verifier,
            bytes32(0x0),
            bytes32(0x0),
            proposalOutputCount,
            outputBlockSpan,
            GameType.wrap(1337),
            OptimismPortal2(payable(address(portal))),
            Claim.wrap(rootClaim),
            l2BlockNumber
        );
        game = new KailuaGame(treasury, genesisTimestamp, l2BlockTime, Duration.wrap(maxClockDuration));
        // Anchoring
        factory.setImplementation(GameType.wrap(1337), treasury);
        anchor = treasury.propose(Claim.wrap(rootClaim), abi.encodePacked(l2BlockNumber, address(treasury)));
        anchor.resolve();
        // Proposals
        factory.setImplementation(GameType.wrap(1337), game);
    }

    function mockFaultProof(
        address payoutRecipient,
        bytes32 l1Head,
        bytes32 acceptedOutputHash,
        bytes32 computedOutputHash,
        uint64 claimedBlockNumber
    ) public view returns (bytes memory proof) {
        bytes32 journalDigest = sha256(
            abi.encodePacked(
                // The address of the recipient of the payout for this proof
                payoutRecipient,
                // No precondition hash
                bytes32(0x0),
                // The L1 head hash containing the safe L2 chain data that may reproduce the L2 head hash.
                l1Head,
                // The latest finalized L2 output root.
                acceptedOutputHash,
                // The L2 output root claim.
                computedOutputHash,
                // The L2 claim block number.
                claimedBlockNumber,
                // The rollup configuration hash
                bytes32(0x0),
                // The FPVM Image ID
                bytes32(0x0)
            )
        );
        bytes32 claimDigest = ReceiptClaimLib.digest(ReceiptClaimLib.ok(bytes32(0x0), journalDigest));

        proof = abi.encodePacked(verifier.SELECTOR(), claimDigest);
    }

    function mockValidityProof(
        address payoutRecipient,
        bytes32 l1Head,
        bytes32 acceptedOutputHash,
        bytes32 computedOutputHash,
        uint64 claimedBlockNumber,
        uint64 proposalOutputCount,
        uint64 outputBlockSpan,
        bytes32 blobsHash
    ) public view returns (bytes memory proof) {
        // Calculate the expected precondition hash if blob data is necessary for proposal
        bytes32 preconditionHash = bytes32(0x0);
        if (proposalOutputCount > 1) {
            uint64 l2BlockNumber = claimedBlockNumber - proposalOutputCount * outputBlockSpan;
            preconditionHash = sha256(abi.encodePacked(l2BlockNumber, proposalOutputCount, outputBlockSpan, blobsHash));
        }

        bytes32 journalDigest = sha256(
            abi.encodePacked(
                // The address of the recipient of the payout for this proof
                payoutRecipient,
                // The blob equivalence precondition hash
                preconditionHash,
                // The L1 head hash containing the safe L2 chain data that may reproduce the L2 head hash.
                l1Head,
                // The latest finalized L2 output root.
                acceptedOutputHash,
                // The L2 output root claim.
                computedOutputHash,
                // The L2 claim block number.
                claimedBlockNumber,
                // The rollup configuration hash
                bytes32(0x0),
                // The FPVM Image ID
                bytes32(0x0)
            )
        );
        bytes32 claimDigest = ReceiptClaimLib.digest(ReceiptClaimLib.ok(bytes32(0x0), journalDigest));

        proof = abi.encodePacked(verifier.SELECTOR(), claimDigest);
    }

    function versionedKZGHash(bytes calldata commitment) external pure returns (bytes32) {
        return KailuaKZGLib.versionedKZGHash(commitment);
    }

    function verifyKZGBlobProof(uint32 index, uint256 value, bytes calldata commitment, bytes calldata proof)
        external
        returns (bool)
    {
        return
            KailuaKZGLib.verifyKZGBlobProof(KailuaKZGLib.versionedKZGHash(commitment), index, value, commitment, proof);
    }

    function modExp(uint256 exponent) external returns (uint256) {
        return KailuaKZGLib.modExp(exponent);
    }
}
