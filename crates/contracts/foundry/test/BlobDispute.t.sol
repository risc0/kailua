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

import "./KailuaTest.t.sol";

contract BlobDisputeTest is KailuaTest {
    KailuaTreasury treasury;
    KailuaGame game;
    KailuaTournament anchor;

    uint256 public constant PROPOSAL_BUFFER_LEN = 21;

    function setUp() public override {
        super.setUp();
        // Deploy dispute contracts
        (treasury, game, anchor) = deployKailua(
            uint256(0x10), // 16 intermediate commitments
            uint256(0x08), // 128 blocks per proposal (8 per commitment)
            sha256(abi.encodePacked(bytes32(0x00))), // arbitrary genesis hash
            uint64(0x0), // genesis
            uint256(block.timestamp), // start l2 from now
            uint256(0x1), // 1-second block times
            uint256(0x5), // 5-second wait
            uint64(0xA) // 10-second dispute timeout
        );
    }

    function test_blobHashes() public {
        vm.warp(
            game.GENESIS_TIME_STAMP() + game.PROPOSAL_TIME_GAP()
                + game.PROPOSAL_OUTPUT_COUNT() * game.OUTPUT_BLOCK_SPAN() * game.L2_BLOCK_TIME()
        );
        // Fail to propose without blob hash
        uint64 anchorIndex = uint64(anchor.gameIndex());
        vm.expectPartialRevert(BlobHashMissing.selector);
        treasury.propose(
            Claim.wrap(0x0001010000010100000010100000101000001010000010100000010100000101),
            abi.encodePacked(uint64(128), anchorIndex, uint64(0))
        );
        // Succeed in proposing with blob hash
        bytes32[] memory blobs = new bytes32[](1);
        blobs[0] = this.versionedKZGHash(BLOB_NZ_COMMIT);
        vm.blobhashes(blobs);
        KailuaTournament proposal_128_0 = treasury.propose(
            Claim.wrap(0x0001010000010100000010100000101000001010000010100000010100000101),
            abi.encodePacked(uint64(128), anchorIndex, uint64(0))
        );
        // Succeed in validating proposed fe
        vm.assertTrue(proposal_128_0.verifyIntermediateOutput(0, BLOB_NZ_VALUE, BLOB_NZ_COMMIT, BLOB_ID_ELEM));
        // Fail to validate under wrong blob
        vm.assertFalse(proposal_128_0.verifyIntermediateOutput(0, 0, BLOB_ID_ELEM, BLOB_ID_ELEM));
    }

    function test_proveNullFault_0() public {
        vm.warp(
            game.GENESIS_TIME_STAMP() + game.PROPOSAL_TIME_GAP()
                + game.PROPOSAL_OUTPUT_COUNT() * game.OUTPUT_BLOCK_SPAN() * game.L2_BLOCK_TIME()
        );
        // Succeed in proposing with blob hash
        bytes32[] memory blobs = new bytes32[](1);
        blobs[0] = this.versionedKZGHash(BLOB_ID_ELEM);
        vm.blobhashes(blobs);
        KailuaTournament proposal_128_0 = treasury.propose(
            Claim.wrap(0x0001010000010100000010100000101000001010000010100000010100000101),
            abi.encodePacked(uint64(128), uint64(anchor.gameIndex()), uint64(0))
        );

        // Fail to prove null fault before io count
        KailuaTournament parent = proposal_128_0.parentGame();
        uint256 proposalOutputCount = treasury.PROPOSAL_OUTPUT_COUNT();
        for (uint256 i = proposalOutputCount; i < 4096; i++) {
            vm.expectRevert(NoConflict.selector);
            parent.proveNullFault(address(this), [uint64(0), uint64(i)], 0, BLOB_ID_ELEM, BLOB_ID_ELEM);
        }

        // Fail to prove null fault at root claim position
        vm.expectRevert(InvalidDisputedClaimIndex.selector);
        parent.proveNullFault(
            address(this), [uint64(0), uint64(proposalOutputCount - 1)], 0, BLOB_ID_ELEM, BLOB_ID_ELEM
        );

        // Succeed to prove null fault before io count
        parent.proveNullFault(address(this), [uint64(0), uint64(0)], 0, BLOB_ID_ELEM, BLOB_ID_ELEM);
    }

    function test_proveNullFault_1() public {
        vm.warp(
            game.GENESIS_TIME_STAMP() + game.PROPOSAL_TIME_GAP()
                + game.PROPOSAL_OUTPUT_COUNT() * game.OUTPUT_BLOCK_SPAN() * game.L2_BLOCK_TIME()
        );
        // Succeed in proposing with blob hash
        bytes32[] memory blobs = new bytes32[](1);
        blobs[0] = this.versionedKZGHash(BLOB_NZ_COMMIT);
        vm.blobhashes(blobs);
        KailuaTournament proposal_128_0 = treasury.propose(
            Claim.wrap(0x0001010000010100000010100000101000001010000010100000010100000101),
            abi.encodePacked(uint64(128), uint64(anchor.gameIndex()), uint64(0))
        );

        // Fail to prove null fault before io count
        KailuaTournament parent = proposal_128_0.parentGame();
        uint256 proposalOutputCount = treasury.PROPOSAL_OUTPUT_COUNT();
        for (uint256 i = 0; i < proposalOutputCount - 1; i++) {
            vm.expectRevert(NoConflict.selector);
            parent.proveNullFault(address(this), [uint64(0), uint64(i)], BLOB_NZ_VALUE, BLOB_NZ_COMMIT, BLOB_ID_ELEM);
        }

        // Fail to prove null fault at root claim position
        vm.expectRevert(InvalidDisputedClaimIndex.selector);
        parent.proveNullFault(
            address(this), [uint64(0), uint64(proposalOutputCount - 1)], BLOB_NZ_VALUE, BLOB_NZ_COMMIT, BLOB_ID_ELEM
        );

        // Fail to prove null fault after blob count
        vm.expectRevert(InvalidDataRemainder.selector);
        parent.proveNullFault(address(this), [uint64(0), uint64(4096 + 2)], BLOB_NZ_VALUE, BLOB_NZ_COMMIT, BLOB_ID_ELEM);

        // Fail to prove null fault with bad blob
        vm.expectRevert("bad proposedOutput kzg");
        parent.proveNullFault(
            address(this), [uint64(0), uint64(proposalOutputCount)], BLOB_NZ_VALUE, BLOB_ID_ELEM, BLOB_ID_ELEM
        );

        // Succeed to prove null fault after io count
        parent.proveNullFault(
            address(this), [uint64(0), uint64(proposalOutputCount)], BLOB_NZ_VALUE, BLOB_NZ_COMMIT, BLOB_ID_ELEM
        );

        // Fail to reprove null fault
        vm.expectRevert(AlreadyProven.selector);
        parent.proveNullFault(
            address(this), [uint64(0), uint64(proposalOutputCount)], BLOB_NZ_VALUE, BLOB_NZ_COMMIT, BLOB_ID_ELEM
        );
    }

    function test_proveNullFault_2() public {
        vm.warp(
            game.GENESIS_TIME_STAMP() + game.PROPOSAL_TIME_GAP()
                + game.PROPOSAL_OUTPUT_COUNT() * game.OUTPUT_BLOCK_SPAN() * game.L2_BLOCK_TIME()
        );
        // Succeed in proposing with blob hash
        bytes32[] memory blobhashes = new bytes32[](1);
        blobhashes[0] = this.versionedKZGHash(BLOB_NZ_COMMIT);
        vm.blobhashes(blobhashes);
        KailuaTournament proposal_128_0 = treasury.propose(
            Claim.wrap(0x0001010000010100000010100000101000001010000010100000010100000101),
            abi.encodePacked(uint64(128), uint64(anchor.gameIndex()), uint64(0))
        );

        vm.warp(
            game.GENESIS_TIME_STAMP() + game.PROPOSAL_TIME_GAP()
                + game.PROPOSAL_OUTPUT_COUNT() * game.OUTPUT_BLOCK_SPAN() * game.L2_BLOCK_TIME() * 2
        );

        // Resolve
        proposal_128_0.resolve();

        // Fail to prove null fault after resolution
        KailuaTournament parent = proposal_128_0.parentGame();
        uint256 proposalOutputCount = treasury.PROPOSAL_OUTPUT_COUNT();
        vm.expectRevert(GameNotInProgress.selector);
        parent.proveNullFault(
            address(this), [uint64(0), uint64(proposalOutputCount)], BLOB_NZ_VALUE, BLOB_NZ_COMMIT, BLOB_ID_ELEM
        );
    }

    function test_proveOutputFault_undisputed_0() public {
        vm.warp(
            game.GENESIS_TIME_STAMP() + game.PROPOSAL_TIME_GAP()
                + game.PROPOSAL_OUTPUT_COUNT() * game.OUTPUT_BLOCK_SPAN() * game.L2_BLOCK_TIME()
        );
        // Succeed in proposing with blob hash
        bytes32[] memory blobhashes = new bytes32[](1);
        blobhashes[0] = this.versionedKZGHash(BLOB_NZ_COMMIT);
        vm.blobhashes(blobhashes);
        KailuaTournament proposal_128_0 = treasury.propose(
            Claim.wrap(0x0001010000010100000010100000101000001010000010100000010100000101),
            abi.encodePacked(uint64(128), uint64(anchor.gameIndex()), uint64(0))
        );

        // Generate mock proof
        bytes32 goodClaim = bytes32(BLOB_NZ_VALUE + KailuaKZGLib.BLS_MODULUS);
        bytes memory proof = mockFaultProof(
            address(this),
            proposal_128_0.l1Head().raw(),
            proposal_128_0.parentGame().rootClaim().raw(),
            goodClaim,
            uint64(0x08)
        );

        // Reject fault proof
        KailuaTournament parent = proposal_128_0.parentGame();
        bytes32 parentClaim = parent.rootClaim().raw();
        bytes[] memory blobCommitments = new bytes[](1);
        blobCommitments[0] = BLOB_NZ_COMMIT;
        bytes[] memory kzgProofs = new bytes[](1);
        kzgProofs[0] = BLOB_ID_ELEM;

        // Reject no conflict fault proof
        vm.expectRevert(NoConflict.selector);
        parent.proveOutputFault(
            address(this),
            [uint64(0), uint64(0)],
            proof,
            parentClaim,
            BLOB_NZ_VALUE,
            goodClaim,
            blobCommitments,
            kzgProofs
        );

        // Reject bad prestate fault proof
        vm.expectRevert("bad acceptedOutput");
        parent.proveOutputFault(
            address(this),
            [uint64(0), uint64(0)],
            proof,
            ~parentClaim,
            BLOB_NZ_VALUE,
            goodClaim,
            blobCommitments,
            kzgProofs
        );

        // Ensure signature is viable
        vm.assertTrue(parent.isViableSignature(proposal_128_0.signature()));

        // Finalize
        vm.warp(block.timestamp + game.MAX_CLOCK_DURATION().raw());
        proposal_128_0.resolve();
    }

    function test_proveOutputFault_undisputed_1() public {
        vm.warp(
            game.GENESIS_TIME_STAMP() + game.PROPOSAL_TIME_GAP()
                + game.PROPOSAL_OUTPUT_COUNT() * game.OUTPUT_BLOCK_SPAN() * game.L2_BLOCK_TIME()
        );
        // Succeed in proposing with blob hash
        bytes32[] memory blobhashes = new bytes32[](1);
        blobhashes[0] = this.versionedKZGHash(BLOB_NZ_COMMIT);
        vm.blobhashes(blobhashes);
        KailuaTournament proposal_128_0 = treasury.propose(
            Claim.wrap(0x0001010000010100000010100000101000001010000010100000010100000101),
            abi.encodePacked(uint64(128), uint64(anchor.gameIndex()), uint64(0))
        );

        // Generate mock proof
        bytes32 goodClaim = ~bytes32(BLOB_NZ_VALUE + KailuaKZGLib.BLS_MODULUS);
        bytes memory proof = mockFaultProof(
            address(this),
            proposal_128_0.l1Head().raw(),
            proposal_128_0.parentGame().rootClaim().raw(),
            goodClaim,
            uint64(0x08)
        );

        // Accept fault proof
        KailuaTournament parent = proposal_128_0.parentGame();
        bytes32 parentClaim = parent.rootClaim().raw();
        bytes[] memory blobCommitments = new bytes[](1);
        blobCommitments[0] = BLOB_NZ_COMMIT;
        bytes[] memory kzgProofs = new bytes[](1);
        kzgProofs[0] = BLOB_ID_ELEM;
        parent.proveOutputFault(
            address(this),
            [uint64(0), uint64(0)],
            proof,
            parentClaim,
            BLOB_NZ_VALUE,
            goodClaim,
            blobCommitments,
            kzgProofs
        );

        // Ensure signature is viable
        vm.assertFalse(parent.isViableSignature(proposal_128_0.signature()));

        // Fail to finalize
        vm.warp(block.timestamp + game.MAX_CLOCK_DURATION().raw());
        vm.expectRevert(NotProven.selector);
        proposal_128_0.resolve();
    }

    function test_proveOutputFault_undisputed_2() public {
        vm.warp(
            game.GENESIS_TIME_STAMP() + game.PROPOSAL_TIME_GAP()
                + game.PROPOSAL_OUTPUT_COUNT() * game.OUTPUT_BLOCK_SPAN() * game.L2_BLOCK_TIME()
        );
        // Succeed in proposing with blob hash
        bytes32[] memory blobhashes = new bytes32[](1);
        blobhashes[0] = this.versionedKZGHash(BLOB_NZ_COMMIT);
        vm.blobhashes(blobhashes);
        KailuaTournament proposal_128_0 = treasury.propose(
            Claim.wrap(0x0001010000010100000010100000101000001010000010100000010100000101),
            abi.encodePacked(uint64(128), uint64(anchor.gameIndex()), uint64(0))
        );

        // Generate mock proof
        bytes32 goodClaim = ~bytes32(BLOB_NZ_VALUE + KailuaKZGLib.BLS_MODULUS);
        bytes memory proof = mockFaultProof(
            address(this), proposal_128_0.l1Head().raw(), bytes32(BLOB_NZ_VALUE), goodClaim, uint64(0x10)
        );

        // Accept fault proof
        KailuaTournament parent = proposal_128_0.parentGame();
        bytes[] memory blobCommitments = new bytes[](2);
        blobCommitments[0] = BLOB_ID_ELEM;
        blobCommitments[1] = BLOB_ID_ELEM;
        bytes[] memory kzgProofs = new bytes[](2);
        kzgProofs[0] = BLOB_ID_ELEM;
        kzgProofs[1] = BLOB_ID_ELEM;

        // Reject bad prestate fault proof
        vm.expectRevert("bad acceptedOutput kzg");
        parent.proveOutputFault(
            address(this),
            [uint64(0), uint64(1)],
            proof,
            ~bytes32(BLOB_NZ_VALUE),
            BLOB_NZ_VALUE,
            goodClaim,
            blobCommitments,
            kzgProofs
        );

        // Reject bad proposed output fault proof
        blobCommitments[0] = BLOB_NZ_COMMIT;
        vm.expectRevert("bad proposedOutput kzg");
        parent.proveOutputFault(
            address(this),
            [uint64(0), uint64(1)],
            proof,
            bytes32(BLOB_NZ_VALUE),
            BLOB_NZ_VALUE,
            goodClaim,
            blobCommitments,
            kzgProofs
        );

        // Accept fault proof
        blobCommitments[1] = BLOB_NZ_COMMIT;
        parent.proveOutputFault(
            address(this),
            [uint64(0), uint64(1)],
            proof,
            bytes32(BLOB_NZ_VALUE),
            BLOB_NZ_VALUE,
            goodClaim,
            blobCommitments,
            kzgProofs
        );

        // Ensure signature is viable
        vm.assertFalse(parent.isViableSignature(proposal_128_0.signature()));

        // Fail to finalize
        vm.warp(block.timestamp + game.MAX_CLOCK_DURATION().raw());
        vm.expectRevert(NotProven.selector);
        proposal_128_0.resolve();
    }

    function test_proveOutputFault_undisputed_3() public {
        vm.warp(
            game.GENESIS_TIME_STAMP() + game.PROPOSAL_TIME_GAP()
                + game.PROPOSAL_OUTPUT_COUNT() * game.OUTPUT_BLOCK_SPAN() * game.L2_BLOCK_TIME()
        );
        // Succeed in proposing with blob hash
        bytes32[] memory blobhashes = new bytes32[](1);
        blobhashes[0] = this.versionedKZGHash(BLOB_NZ_COMMIT);
        vm.blobhashes(blobhashes);
        KailuaTournament proposal_128_0 = treasury.propose(
            Claim.wrap(0x0001010000010100000010100000101000001010000010100000010100000101),
            abi.encodePacked(uint64(128), uint64(anchor.gameIndex()), uint64(0))
        );

        // Generate mock proof
        bytes32 goodClaim = ~proposal_128_0.rootClaim().raw();
        bytes memory proof = mockFaultProof(
            address(this), proposal_128_0.l1Head().raw(), bytes32(BLOB_NZ_VALUE), goodClaim, uint64(0x80)
        );

        // Accept fault proof
        KailuaTournament parent = proposal_128_0.parentGame();
        bytes[] memory blobCommitments = new bytes[](1);
        blobCommitments[0] = BLOB_NZ_COMMIT;
        bytes[] memory kzgProofs = new bytes[](1);
        kzgProofs[0] = BLOB_ID_ELEM;
        parent.proveOutputFault(
            address(this),
            [uint64(0), uint64(0x0f)],
            proof,
            bytes32(BLOB_NZ_VALUE),
            KailuaKZGLib.hashToFe(proposal_128_0.rootClaim().raw()),
            goodClaim,
            blobCommitments,
            kzgProofs
        );

        // Ensure signature is viable
        vm.assertFalse(parent.isViableSignature(proposal_128_0.signature()));

        // Fail to finalize
        vm.warp(block.timestamp + game.MAX_CLOCK_DURATION().raw());
        vm.expectRevert(NotProven.selector);
        proposal_128_0.resolve();
    }

    function test_proveValidity() public {
        vm.warp(
            game.GENESIS_TIME_STAMP() + game.PROPOSAL_TIME_GAP()
                + game.PROPOSAL_OUTPUT_COUNT() * game.OUTPUT_BLOCK_SPAN() * game.L2_BLOCK_TIME()
        );
        // honest proposal
        bytes32[] memory blobs = new bytes32[](1);
        blobs[0] = this.versionedKZGHash(BLOB_NZ_COMMIT);
        vm.blobhashes(blobs);
        KailuaTournament proposal_128_0 = treasury.propose(
            Claim.wrap(0x0001010000010100000010100000101000001010000010100000010100000100),
            abi.encodePacked(uint64(128), uint64(anchor.gameIndex()), uint64(0))
        );

        vm.warp(
            game.GENESIS_TIME_STAMP() + game.PROPOSAL_TIME_GAP()
                + game.PROPOSAL_OUTPUT_COUNT() * game.OUTPUT_BLOCK_SPAN() * game.L2_BLOCK_TIME() * 2
        );
        proposal_128_0.resolve();
        // Generate mock proof
        bytes memory proof = mockValidityProof(
            address(this),
            proposal_128_0.l1Head().raw(),
            anchor.rootClaim().raw(),
            proposal_128_0.rootClaim().raw(),
            uint64(proposal_128_0.l2BlockNumber()),
            uint64(treasury.PROPOSAL_OUTPUT_COUNT()),
            uint64(treasury.OUTPUT_BLOCK_SPAN()),
            proposal_128_0.blobsHash()
        );

        // Reject validity proof after resolution
        vm.expectRevert(GameNotInProgress.selector);
        anchor.proveValidity(address(this), uint64(0), proof);

        // honest proposal
        vm.blobhashes(blobs);
        KailuaTournament proposal_256_0 = treasury.propose(
            Claim.wrap(0x0001010000010100000010100000101000001010000010100000010100000100),
            abi.encodePacked(uint64(256), uint64(proposal_128_0.gameIndex()), uint64(0))
        );

        // Generate mock proof
        proof = mockValidityProof(
            address(this),
            proposal_256_0.l1Head().raw(),
            proposal_128_0.rootClaim().raw(),
            proposal_256_0.rootClaim().raw(),
            uint64(proposal_256_0.l2BlockNumber()),
            uint64(treasury.PROPOSAL_OUTPUT_COUNT()),
            uint64(treasury.OUTPUT_BLOCK_SPAN()),
            proposal_256_0.blobsHash()
        );

        // Accept validity proof
        proposal_128_0.proveValidity(address(this), uint64(0), proof);

        // Reject repeat validity proof
        vm.expectRevert(AlreadyProven.selector);
        proposal_128_0.proveValidity(address(this), uint64(0), proof);

        // Resolve
        proposal_256_0.resolve();
    }
}
