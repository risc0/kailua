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

contract ClaimDisputeTest is KailuaTest {
    KailuaTreasury treasury;
    KailuaGame game;
    KailuaTournament anchor;

    uint256 public constant PROPOSAL_BUFFER_LEN = 21;

    function setUp() public override {
        super.setUp();
        // Deploy dispute contracts
        (treasury, game, anchor) = deployKailua(
            uint256(0x1), // no intermediate commitments
            uint256(0x80), // 128 blocks per proposal
            sha256(abi.encodePacked(bytes32(0x00))), // arbitrary block hash
            uint64(0x0), // genesis
            uint256(block.timestamp), // start l2 from now
            uint256(0x1), // 1-second block times
            uint256(0x5), // 5-second wait
            uint64(0xA) // 10-second dispute timeout
        );
    }

    function test_getChallengerDuration() public {
        vm.warp(
            game.GENESIS_TIME_STAMP() + game.PROPOSAL_TIME_GAP()
                + game.PROPOSAL_OUTPUT_COUNT() * game.OUTPUT_BLOCK_SPAN() * game.L2_BLOCK_TIME()
        );
        // Succeed to propose after proposal time gap
        KailuaTournament proposal_128_0 = treasury.propose(
            Claim.wrap(0x0001010000010100000010100000101000001010000010100000010100000101),
            abi.encodePacked(uint64(128), uint64(anchor.gameIndex()), uint64(0))
        );

        // Fail to resolve before dispute timeout
        vm.expectRevert();
        proposal_128_0.resolve();

        // Fail to resolve just before dispute timeout
        vm.warp(block.timestamp + game.MAX_CLOCK_DURATION().raw() - 1);
        vm.expectRevert();
        proposal_128_0.resolve();

        // Resolve after dispute timeout
        vm.warp(block.timestamp + 1);
        proposal_128_0.resolve();
    }

    function test_eliminate() public {
        vm.warp(
            game.GENESIS_TIME_STAMP() + game.PROPOSAL_TIME_GAP()
                + game.PROPOSAL_OUTPUT_COUNT() * game.OUTPUT_BLOCK_SPAN() * game.L2_BLOCK_TIME()
        );
        // Succeed to propose after proposal time gap
        KailuaTournament proposal_128_0 = treasury.propose(
            Claim.wrap(0x0001010000010100000010100000101000001010000010100000010100000101),
            abi.encodePacked(uint64(128), uint64(anchor.gameIndex()), uint64(0))
        );
        vm.warp(
            game.GENESIS_TIME_STAMP() + game.PROPOSAL_TIME_GAP()
                + game.PROPOSAL_OUTPUT_COUNT() * game.OUTPUT_BLOCK_SPAN() * game.L2_BLOCK_TIME() * 2
        );
        // Succeed to propose after proposal time gap
        KailuaTournament proposal_256_0 = treasury.propose(
            Claim.wrap(0x0001010000010100000010100000101000001010000010100000010100000101),
            abi.encodePacked(uint64(256), uint64(proposal_128_0.gameIndex()), uint64(0))
        );

        // Fail to eliminate from random address
        vm.expectPartialRevert(Blacklisted.selector);
        treasury.eliminate(address(proposal_256_0), address(this));

        // Succeed to eliminate from parent address
        vm.startPrank(address(proposal_128_0));
        treasury.eliminate(address(proposal_256_0), address(this));

        // Fail to double eliminate
        vm.expectRevert(AlreadyEliminated.selector);
        treasury.eliminate(address(proposal_256_0), address(this));
        vm.stopPrank();

        // Fail to propose again after elimination
        uint64 parentIndex = uint64(proposal_128_0.gameIndex());
        vm.expectRevert(BadAuth.selector);
        treasury.propose(
            Claim.wrap(0x0001010000010100000010100000101000001010000010100000010100000100),
            abi.encodePacked(uint64(256), parentIndex, uint64(0))
        );

        (KailuaTreasury new_treasury, KailuaGame new_game, KailuaTournament new_anchor) = deployKailua(
            uint256(0x1), // no intermediate commitments
            uint256(0x80), // 128 blocks per proposal
            sha256(abi.encodePacked(bytes32(0x00))), // arbitrary block hash
            uint64(0x0), // genesis
            uint256(block.timestamp), // start l2 from now
            uint256(0x1), // 1-second block times
            uint256(0x5), // 5-second wait
            uint64(0xA) // 10-second dispute timeout
        );

        vm.warp(
            new_game.GENESIS_TIME_STAMP() + new_game.PROPOSAL_TIME_GAP()
                + new_game.PROPOSAL_OUTPUT_COUNT() * new_game.OUTPUT_BLOCK_SPAN() * new_game.L2_BLOCK_TIME()
        );
        // Succeed to propose after proposal time gap
        KailuaTournament new_proposal_128_0 = new_treasury.propose(
            Claim.wrap(0x0001010000010100000010100000101000001010000010100000010100000101),
            abi.encodePacked(uint64(128), uint64(new_anchor.gameIndex()), uint64(0))
        );
        vm.warp(
            new_game.GENESIS_TIME_STAMP() + new_game.PROPOSAL_TIME_GAP()
                + new_game.PROPOSAL_OUTPUT_COUNT() * new_game.OUTPUT_BLOCK_SPAN() * new_game.L2_BLOCK_TIME() * 2
        );
        // Succeed to propose after proposal time gap
        KailuaTournament new_proposal_256_0 = new_treasury.propose(
            Claim.wrap(0x0001010000010100000010100000101000001010000010100000010100000101),
            abi.encodePacked(uint64(256), uint64(new_proposal_128_0.gameIndex()), uint64(0))
        );

        // Fail to eliminate from parent address
        vm.startPrank(address(new_proposal_128_0));
        vm.expectRevert(NotProposed.selector);
        treasury.eliminate(address(new_proposal_256_0), address(this));
        vm.stopPrank();
    }

    function test_proveValidity_undisputed() public {
        vm.warp(
            game.GENESIS_TIME_STAMP() + game.PROPOSAL_TIME_GAP()
                + game.PROPOSAL_OUTPUT_COUNT() * game.OUTPUT_BLOCK_SPAN() * game.L2_BLOCK_TIME()
        );
        // Succeed to propose after proposal time gap
        KailuaTournament proposal_128_0 = treasury.propose(
            Claim.wrap(0x0001010000010100000010100000101000001010000010100000010100000101),
            abi.encodePacked(uint64(128), uint64(anchor.gameIndex()), uint64(0))
        );

        // Generate mock proof
        bytes memory proof = mockValidityProof(
            address(this),
            proposal_128_0.l1Head().raw(),
            proposal_128_0.parentGame().rootClaim().raw(),
            proposal_128_0.rootClaim().raw(),
            uint64(proposal_128_0.l2BlockNumber()),
            uint64(proposal_128_0.PROPOSAL_OUTPUT_COUNT()),
            uint64(proposal_128_0.OUTPUT_BLOCK_SPAN()),
            proposal_128_0.blobsHash()
        );

        // Reject fault proof that shows validity
        try proposal_128_0.parentGame().proveOutputFault(
            address(this),
            [uint64(0), uint64(0)],
            proof,
            proposal_128_0.parentGame().rootClaim().raw(),
            KailuaKZGLib.hashToFe(proposal_128_0.rootClaim().raw()),
            proposal_128_0.rootClaim().raw(),
            new bytes[](0),
            new bytes[](0)
        ) {
            vm.assertTrue(false);
        } catch (bytes memory reason) {
            vm.assertEq(reason, abi.encodePacked(NoConflict.selector));
        }

        // Refuse to finalize before timeout
        vm.expectRevert();
        proposal_128_0.resolve();

        // Accept validity proof
        proposal_128_0.parentGame().proveValidity(address(this), uint64(0), proof);

        // Ensure signature is unviable
        vm.assertTrue(proposal_128_0.parentGame().isViableSignature(proposal_128_0.signature()));

        // Finalize
        proposal_128_0.resolve();
    }

    function test_proveValidity() public {
        vm.warp(
            game.GENESIS_TIME_STAMP() + game.PROPOSAL_TIME_GAP()
                + game.PROPOSAL_OUTPUT_COUNT() * game.OUTPUT_BLOCK_SPAN() * game.L2_BLOCK_TIME()
        );
        // honest proposal
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
        bytes memory proof = mockFaultProof(
            address(this),
            proposal_128_0.l1Head().raw(),
            anchor.rootClaim().raw(),
            proposal_128_0.rootClaim().raw(),
            uint64(proposal_128_0.l2BlockNumber())
        );

        // Reject validity proof after resolution
        vm.expectRevert(GameNotInProgress.selector);
        anchor.proveValidity(address(this), uint64(0), proof);

        // honest proposal
        KailuaTournament proposal_256_0 = treasury.propose(
            Claim.wrap(0x0001010000010100000010100000101000001010000010100000010100000100),
            abi.encodePacked(uint64(256), uint64(proposal_128_0.gameIndex()), uint64(0))
        );

        // bad proposal
        vm.startPrank(address(0x01));
        KailuaTournament proposal_256_1 = treasury.propose(
            Claim.wrap(0x0001010000010100000010100000101000001010000010100000010100000101),
            abi.encodePacked(uint64(256), uint64(proposal_128_0.gameIndex()), uint64(0))
        );
        vm.stopPrank();

        // Generate mock proof
        proof = mockFaultProof(
            address(this),
            proposal_256_0.l1Head().raw(),
            proposal_128_0.rootClaim().raw(),
            proposal_256_0.rootClaim().raw(),
            uint64(proposal_256_0.l2BlockNumber())
        );

        // Accept validity proof
        proposal_128_0.proveValidity(address(this), uint64(0), proof);

        // Reject repeat validity proof
        vm.expectRevert(AlreadyProven.selector);
        proposal_128_0.proveValidity(address(this), uint64(0), proof);

        // Reject resolve for bad proposal
        vm.expectRevert(ProvenFaulty.selector);
        proposal_256_1.resolve();

        // Resolve honest proposal
        proposal_256_0.resolve();
    }

    function test_prove_resolved() public {
        vm.warp(
            game.GENESIS_TIME_STAMP() + game.PROPOSAL_TIME_GAP()
            + game.PROPOSAL_OUTPUT_COUNT() * game.OUTPUT_BLOCK_SPAN() * game.L2_BLOCK_TIME()
        );
        // honest proposal
        KailuaTournament proposal_128_0 = treasury.propose(
            Claim.wrap(0x0001010000010100000010100000101000001010000010100000010100000100),
            abi.encodePacked(uint64(128), uint64(anchor.gameIndex()), uint64(0))
        );

        vm.warp(
            game.GENESIS_TIME_STAMP() + game.PROPOSAL_TIME_GAP()
            + game.PROPOSAL_OUTPUT_COUNT() * game.OUTPUT_BLOCK_SPAN() * game.L2_BLOCK_TIME() * 2
        );
        proposal_128_0.resolve();

        // honest proposal
        KailuaTournament proposal_256_0 = treasury.propose(
            Claim.wrap(0x0001010000010100000010100000101000001010000010100000010100000100),
            abi.encodePacked(uint64(256), uint64(proposal_128_0.gameIndex()), uint64(0))
        );

        // bad proposal
        vm.startPrank(address(0x01));
        KailuaTournament proposal_256_1 = treasury.propose(
            Claim.wrap(0x0001010000010100000010100000101000001010000010100000010100000101),
            abi.encodePacked(uint64(256), uint64(proposal_128_0.gameIndex()), uint64(0))
        );
        vm.stopPrank();

        // Generate mock proof
        bytes memory proof = mockFaultProof(
            address(this),
            proposal_256_0.l1Head().raw(),
            proposal_128_0.rootClaim().raw(),
            proposal_256_0.rootClaim().raw(),
            uint64(proposal_256_0.l2BlockNumber())
        );

        // Accept validity proof
        proposal_128_0.proveValidity(address(this), uint64(0), proof);

        // Resolve honest proposal
        proposal_256_0.resolve();
        vm.expectRevert(GameNotInProgress.selector);
        proposal_256_0.getChallengerDuration(block.timestamp);

        // Reject output fault proof after resolution
        bytes32 parentRoot = proposal_128_0.rootClaim().raw();
        uint256 badRoot = KailuaKZGLib.hashToFe(proposal_256_1.rootClaim().raw());
        bytes32 goodClaim = proposal_256_0.rootClaim().raw();
        vm.expectRevert(ClaimAlreadyResolved.selector);
        proposal_128_0.proveOutputFault(
            address(this), [uint64(1), uint64(0)], proof, parentRoot, badRoot, goodClaim, new bytes[](0), new bytes[](0)
        );

        // Reject null fault proof after resolution
        vm.expectRevert(ClaimAlreadyResolved.selector);
        proposal_128_0.proveNullFault(address(this), [uint64(1), uint64(0)], 0, BLOB_ID_ELEM, BLOB_ID_ELEM);

        // Mock validity proof
        proof = mockFaultProof(
            address(this),
            proposal_256_1.l1Head().raw(),
            proposal_128_0.rootClaim().raw(),
            proposal_256_1.rootClaim().raw(),
            uint64(proposal_256_1.l2BlockNumber())
        );

        // Reject validity proof after resolution
        vm.expectRevert(ClaimAlreadyResolved.selector);
        proposal_128_0.proveValidity(address(this), uint64(1), proof);
    }

    function test_proveOutputFault_range() public {
        vm.warp(
            game.GENESIS_TIME_STAMP() + game.PROPOSAL_TIME_GAP()
                + game.PROPOSAL_OUTPUT_COUNT() * game.OUTPUT_BLOCK_SPAN() * game.L2_BLOCK_TIME()
        );
        // Succeed to propose after proposal time gap
        KailuaTournament proposal_128_0 = treasury.propose(
            Claim.wrap(0x0001010000010100000010100000101000001010000010100000010100000101),
            abi.encodePacked(uint64(128), uint64(anchor.gameIndex()), uint64(0))
        );

        // Generate mock proof
        bytes32 goodClaim = bytes32(uint256(proposal_128_0.rootClaim().raw()) + KailuaKZGLib.BLS_MODULUS);
        bytes memory proof = mockFaultProof(
            address(this),
            proposal_128_0.l1Head().raw(),
            proposal_128_0.parentGame().rootClaim().raw(),
            goodClaim,
            uint64(proposal_128_0.l2BlockNumber())
        );

        // Reject fault proof
        bytes32 parentRoot = anchor.rootClaim().raw();
        bytes32 badRoot = proposal_128_0.rootClaim().raw();
        vm.expectRevert(InvalidDisputedClaimIndex.selector);
        anchor.proveOutputFault(
            address(this),
            [uint64(0), uint64(1)],
            proof,
            parentRoot,
            KailuaKZGLib.hashToFe(badRoot),
            goodClaim,
            new bytes[](0),
            new bytes[](0)
        );
    }

    function test_proveOutputFault_late() public {
        vm.warp(
            game.GENESIS_TIME_STAMP() + game.PROPOSAL_TIME_GAP()
                + game.PROPOSAL_OUTPUT_COUNT() * game.OUTPUT_BLOCK_SPAN() * game.L2_BLOCK_TIME()
        );
        // Succeed to propose after proposal time gap
        KailuaTournament proposal_128_0 = treasury.propose(
            Claim.wrap(0x0001010000010100000010100000101000001010000010100000010100000101),
            abi.encodePacked(uint64(128), uint64(anchor.gameIndex()), uint64(0))
        );

        // Finalize claim
        vm.warp(
            game.GENESIS_TIME_STAMP() + game.PROPOSAL_TIME_GAP()
                + game.PROPOSAL_OUTPUT_COUNT() * game.OUTPUT_BLOCK_SPAN() * game.L2_BLOCK_TIME() * 2
        );
        proposal_128_0.resolve();

        // Generate mock proof
        bytes32 goodClaim = bytes32(uint256(proposal_128_0.rootClaim().raw()) + KailuaKZGLib.BLS_MODULUS);
        bytes memory proof = mockFaultProof(
            address(this),
            proposal_128_0.l1Head().raw(),
            proposal_128_0.parentGame().rootClaim().raw(),
            goodClaim,
            uint64(proposal_128_0.l2BlockNumber())
        );

        // Reject fault proof
        bytes32 parentRoot = anchor.rootClaim().raw();
        bytes32 badRoot = proposal_128_0.rootClaim().raw();
        vm.expectRevert(GameNotInProgress.selector);
        anchor.proveOutputFault(
            address(this),
            [uint64(0), uint64(0)],
            proof,
            parentRoot,
            KailuaKZGLib.hashToFe(badRoot),
            goodClaim,
            new bytes[](0),
            new bytes[](0)
        );
    }

    function test_proveOutputFault_undisputed() public {
        // Time for at most two proposals
        vm.warp(
            game.GENESIS_TIME_STAMP() + game.PROPOSAL_TIME_GAP()
                + game.PROPOSAL_OUTPUT_COUNT() * game.OUTPUT_BLOCK_SPAN() * game.L2_BLOCK_TIME() * 2
        );
        // Succeed to propose after proposal time gap
        KailuaTournament proposal_128_0 = treasury.propose(
            Claim.wrap(0x0001010000010100000010100000101000001010000010100000010100000101),
            abi.encodePacked(uint64(128), uint64(anchor.gameIndex()), uint64(0))
        );

        // Generate mock proof
        bytes32 goodClaim = bytes32(uint256(proposal_128_0.rootClaim().raw()) + KailuaKZGLib.BLS_MODULUS);
        bytes memory proof = mockFaultProof(
            address(this),
            proposal_128_0.l1Head().raw(),
            proposal_128_0.parentGame().rootClaim().raw(),
            goodClaim,
            uint64(proposal_128_0.l2BlockNumber())
        );

        // Accept fault proof
        proposal_128_0.parentGame().proveOutputFault(
            address(this),
            [uint64(0), uint64(0)],
            proof,
            proposal_128_0.parentGame().rootClaim().raw(),
            KailuaKZGLib.hashToFe(proposal_128_0.rootClaim().raw()),
            goodClaim,
            new bytes[](0),
            new bytes[](0)
        );

        // Ensure signature is unviable
        vm.assertFalse(proposal_128_0.parentGame().isViableSignature(proposal_128_0.signature()));

        // Fail to finalize disproven claim
        vm.expectRevert();
        proposal_128_0.resolve();

        // Fail to repeat disproven claim
        uint64 parentIndex = uint64(anchor.gameIndex());
        vm.expectRevert(ProvenFaulty.selector);
        treasury.propose(
            Claim.wrap(0x0001010000010100000010100000101000001010000010100000010100000101),
            abi.encodePacked(uint64(128), parentIndex, uint64(1))
        );

        // Fail to recover proposer bond due to hanging proposal
        vm.expectRevert(GameNotResolved.selector);
        treasury.claimProposerBond();

        // Eliminate player
        proposal_128_0.parentGame().pruneChildren(1);

        // Fail to recover proposer bond due to elimination
        vm.expectRevert(AlreadyEliminated.selector);
        treasury.claimProposerBond();
    }

    function test_proveOutputFault_disputed() public {
        uint64 parentIndex = uint64(anchor.gameIndex());

        for (uint256 i = 1; i < PROPOSAL_BUFFER_LEN; i++) {
            uint64 blockHeight = uint64(128 * i);

            vm.warp(
                game.GENESIS_TIME_STAMP() + game.PROPOSAL_TIME_GAP()
                    + game.PROPOSAL_OUTPUT_COUNT() * game.OUTPUT_BLOCK_SPAN() * game.L2_BLOCK_TIME() * i
            );

            KailuaTournament[PROPOSAL_BUFFER_LEN] memory proposals;
            for (uint256 j = 1; j < PROPOSAL_BUFFER_LEN; j++) {
                vm.startPrank(address(bytes20(uint160(100000 * i + j))));
                if (j == i) {
                    // Send successful proposal
                    proposals[j] = treasury.propose(
                        Claim.wrap(0x0001010000010100000010100000101000001010000010100000010100000101),
                        abi.encodePacked(blockHeight, parentIndex, uint64(0))
                    );
                } else {
                    proposals[j] = treasury.propose(
                        Claim.wrap(sha256(abi.encodePacked(bytes32(j)))),
                        abi.encodePacked(blockHeight, parentIndex, uint64(0))
                    );
                }
                vm.stopPrank();
            }

            // Fail to resolve without dispute resolution
            vm.warp(block.timestamp + game.MAX_CLOCK_DURATION().raw());
            for (uint256 j = 1; j < PROPOSAL_BUFFER_LEN; j++) {
                vm.expectRevert();
                proposals[j].resolve();
            }

            // Publish late proposal
            vm.startPrank(address(bytes20(uint160(100000 * i))));
            proposals[0] = treasury.propose(
                Claim.wrap(sha256(abi.encodePacked(bytes32(0)))), abi.encodePacked(blockHeight, parentIndex, uint64(0))
            );
            vm.stopPrank();

            // Submit fault proofs
            for (uint256 j = 1; j < PROPOSAL_BUFFER_LEN; j++) {
                // Don't prove the ith proposal faulty
                if (j == i) {
                    continue;
                }

                // Generate mock proof
                bytes memory proof = mockFaultProof(
                    address(this),
                    proposals[j].l1Head().raw(),
                    proposals[j].parentGame().rootClaim().raw(),
                    proposals[i].rootClaim().raw(),
                    uint64(proposals[j].l2BlockNumber())
                );

                // Accept fault proof
                proposals[j].parentGame().proveOutputFault(
                    address(this),
                    [uint64(j - 1), uint64(0)],
                    proof,
                    proposals[j].parentGame().rootClaim().raw(),
                    KailuaKZGLib.hashToFe(proposals[j].rootClaim().raw()),
                    proposals[i].rootClaim().raw(),
                    new bytes[](0),
                    new bytes[](0)
                );

                // Reject repeat fault proof
                KailuaTournament parent = proposals[j].parentGame();
                bytes32 parentRoot = parent.rootClaim().raw();
                bytes32 badRoot = proposals[j].rootClaim().raw();
                bytes32 goodRoot = proposals[i].rootClaim().raw();
                vm.expectRevert(AlreadyProven.selector);
                parent.proveOutputFault(
                    address(this),
                    [uint64(j - 1), uint64(0)],
                    proof,
                    parentRoot,
                    KailuaKZGLib.hashToFe(badRoot),
                    goodRoot,
                    new bytes[](0),
                    new bytes[](0)
                );

                // Ensure signature is unviable
                vm.assertFalse(proposals[j].parentGame().isViableSignature(proposals[j].signature()));
            }

            // Fail to resolve any non-canonical proposal
            for (uint256 j = 0; j < PROPOSAL_BUFFER_LEN; j++) {
                if (i == j) {
                    continue;
                }
                vm.expectRevert();
                proposals[j].resolve();
            }

            // Finalize canonical proposal
            proposals[i].resolve();

            // Fail to resolve any proposal after correct resolution
            for (uint256 j = 0; j < PROPOSAL_BUFFER_LEN; j++) {
                vm.expectRevert();
                proposals[j].resolve();
            }

            // Update parent
            parentIndex = uint64(proposals[i].gameIndex());
        }

        // Validate eliminations count
        uint256 eliminationsCount = (PROPOSAL_BUFFER_LEN - 1) * (PROPOSAL_BUFFER_LEN - 2);
        vm.expectRevert();
        treasury.eliminations(address(this), eliminationsCount);
        // This should not revert
        treasury.eliminations(address(this), eliminationsCount - 1);

        // Claim elimination bonds
        treasury.claimEliminationBonds(eliminationsCount);
        vm.assertEq(treasury.eliminationsPaid(address(this)), eliminationsCount);
    }

    function test_proveOutputFault_duplicates() public {
        uint64 parentIndex = uint64(anchor.gameIndex());

        for (uint256 i = 1; i < PROPOSAL_BUFFER_LEN; i++) {
            uint64 blockHeight = uint64(128 * i);
            uint64 dupeCtr = 0;

            vm.warp(
                game.GENESIS_TIME_STAMP() + game.PROPOSAL_TIME_GAP()
                    + game.PROPOSAL_OUTPUT_COUNT() * game.OUTPUT_BLOCK_SPAN() * game.L2_BLOCK_TIME() * i
            );

            KailuaTournament[PROPOSAL_BUFFER_LEN] memory proposals;
            for (uint256 j = 1; j < PROPOSAL_BUFFER_LEN; j++) {
                vm.startPrank(address(bytes20(uint160(10000 * i + j))));
                if (j % i == 0) {
                    // Send successful proposal
                    proposals[j] = treasury.propose(
                        Claim.wrap(0x0001010000010100000010100000101000001010000010100000010100000101),
                        abi.encodePacked(blockHeight, parentIndex, dupeCtr++)
                    );
                } else {
                    proposals[j] = treasury.propose(
                        Claim.wrap(sha256(abi.encodePacked(bytes32(j)))),
                        abi.encodePacked(blockHeight, parentIndex, uint64(0))
                    );
                }
                vm.stopPrank();
            }

            // Fail to resolve without dispute resolution
            vm.warp(block.timestamp + game.MAX_CLOCK_DURATION().raw());
            if (i > 1) {
                for (uint256 j = 1; j < PROPOSAL_BUFFER_LEN; j++) {
                    vm.expectRevert();
                    proposals[j].resolve();
                }
            }

            // Publish late proposal
            vm.startPrank(address(bytes20(uint160(100000 * i))));
            proposals[0] = treasury.propose(
                Claim.wrap(sha256(abi.encodePacked(bytes32(0)))), abi.encodePacked(blockHeight, parentIndex, uint64(0))
            );
            vm.stopPrank();

            // Submit fault proofs
            for (uint256 j = 1; j < PROPOSAL_BUFFER_LEN; j++) {
                // Don't prove the dupe proposal faulty
                if (j % i == 0) {
                    continue;
                }

                // Generate mock proof
                bytes memory proof = mockFaultProof(
                    address(this),
                    proposals[j].l1Head().raw(),
                    proposals[j].parentGame().rootClaim().raw(),
                    proposals[i].rootClaim().raw(),
                    uint64(proposals[j].l2BlockNumber())
                );

                // Accept fault proof
                proposals[j].parentGame().proveOutputFault(
                    address(this),
                    [uint64(j - 1), uint64(0)],
                    proof,
                    proposals[j].parentGame().rootClaim().raw(),
                    KailuaKZGLib.hashToFe(proposals[j].rootClaim().raw()),
                    proposals[i].rootClaim().raw(),
                    new bytes[](0),
                    new bytes[](0)
                );

                // Ensure signature is unviable
                vm.assertFalse(proposals[j].parentGame().isViableSignature(proposals[j].signature()));
            }

            // Fail to resolve any non-canonical proposal
            for (uint256 j = 0; j < PROPOSAL_BUFFER_LEN; j++) {
                if (i == j) {
                    continue;
                }
                vm.expectRevert();
                proposals[j].resolve();
            }

            // Finalize canonical proposal
            proposals[i].resolve();

            // Fail to resolve any proposal after correct resolution
            for (uint256 j = 0; j < PROPOSAL_BUFFER_LEN; j++) {
                vm.expectRevert();
                proposals[j].resolve();
            }

            // Update parent
            parentIndex = uint64(proposals[i].gameIndex());
        }
    }

    function test_pruneChildren_duplicates() public {
        uint64 parentIndex = uint64(anchor.gameIndex());

        for (uint256 i = 1; i < PROPOSAL_BUFFER_LEN; i++) {
            uint64 blockHeight = uint64(128 * i);
            uint64 honestDupeCtr = 0;
            uint64 badDupeCtr = 0;

            vm.warp(
                game.GENESIS_TIME_STAMP() + game.PROPOSAL_TIME_GAP()
                    + game.PROPOSAL_OUTPUT_COUNT() * game.OUTPUT_BLOCK_SPAN() * game.L2_BLOCK_TIME() * i
            );

            KailuaTournament[PROPOSAL_BUFFER_LEN] memory proposals;
            for (uint256 j = 1; j < PROPOSAL_BUFFER_LEN; j++) {
                vm.startPrank(address(bytes20(uint160(10000 * i + j))));
                if (j % i == 0) {
                    // Send successful proposal
                    proposals[j] = treasury.propose(
                        Claim.wrap(0x0001010000010100000010100000101000001010000010100000010100000101),
                        abi.encodePacked(blockHeight, parentIndex, honestDupeCtr++)
                    );
                } else {
                    proposals[j] = treasury.propose(
                        Claim.wrap(0x0001010000010100000010100000101000001010000010100000010100000100),
                        abi.encodePacked(blockHeight, parentIndex, badDupeCtr++)
                    );
                }
                vm.stopPrank();
            }

            // Fail to resolve without dispute resolution
            vm.warp(block.timestamp + game.MAX_CLOCK_DURATION().raw());
            if (i > 1) {
                for (uint256 j = 1; j < PROPOSAL_BUFFER_LEN; j++) {
                    vm.expectRevert();
                    proposals[j].resolve();
                }
            }

            // Publish late proposal
            vm.startPrank(address(bytes20(uint160(100000 * i))));
            proposals[0] = treasury.propose(
                Claim.wrap(sha256(abi.encodePacked(bytes32(0)))), abi.encodePacked(blockHeight, parentIndex, uint64(0))
            );
            vm.stopPrank();

            // Queue some duplicates
            if (i > 2) {
                proposals[i].parentGame().pruneChildren(i - 2);
            }

            // Submit fault proof
            for (uint256 j = 1; j < PROPOSAL_BUFFER_LEN; j++) {
                // Don't prove the dupe proposal faulty
                if (j % i == 0) {
                    continue;
                }

                // Generate mock proof
                bytes memory proof = mockFaultProof(
                    address(this),
                    proposals[j].l1Head().raw(),
                    proposals[j].parentGame().rootClaim().raw(),
                    proposals[i].rootClaim().raw(),
                    uint64(proposals[j].l2BlockNumber())
                );

                // Accept fault proof
                proposals[j].parentGame().proveOutputFault(
                    address(this),
                    [uint64(j - 1), uint64(0)],
                    proof,
                    proposals[j].parentGame().rootClaim().raw(),
                    KailuaKZGLib.hashToFe(proposals[j].rootClaim().raw()),
                    proposals[i].rootClaim().raw(),
                    new bytes[](0),
                    new bytes[](0)
                );

                // Ensure signature is unviable
                vm.assertFalse(proposals[j].parentGame().isViableSignature(proposals[j].signature()));

                break;
            }

            // Prune all contradictions
            while (address(proposals[i].parentGame().pruneChildren(1)) != address(proposals[i])) {}

            // Finalize canonical proposal
            proposals[i].resolve();

            // Fail to resolve any proposal after correct resolution
            for (uint256 j = 0; j < PROPOSAL_BUFFER_LEN; j++) {
                vm.expectRevert();
                proposals[j].resolve();
            }

            // Update parent
            parentIndex = uint64(proposals[i].gameIndex());
        }
    }

    function test_pruneChildren_contenders() public {
        vm.warp(
            game.GENESIS_TIME_STAMP() + game.PROPOSAL_TIME_GAP()
                + game.PROPOSAL_OUTPUT_COUNT() * game.OUTPUT_BLOCK_SPAN() * game.L2_BLOCK_TIME()
        );

        // honest proposal
        KailuaTournament proposal_128_0 = treasury.propose(
            Claim.wrap(0x0001010000010100000010100000101000001010000010100000010100000100),
            abi.encodePacked(uint64(128), uint64(anchor.gameIndex()), uint64(0))
        );

        // bad duplicate proposal
        KailuaTournament[PROPOSAL_BUFFER_LEN] memory proposals_128;
        for (uint256 i = 0; i < PROPOSAL_BUFFER_LEN; i++) {
            vm.startPrank(address(bytes20(uint160(10000 * i + 1))));
            proposals_128[i] = treasury.propose(
                Claim.wrap(0x0001010000010100000010100000101000001010000010100000010100000101),
                abi.encodePacked(uint64(128), uint64(anchor.gameIndex()), uint64(i))
            );
            vm.stopPrank();
        }

        vm.warp(
            game.GENESIS_TIME_STAMP() + game.PROPOSAL_TIME_GAP()
                + game.PROPOSAL_OUTPUT_COUNT() * game.OUTPUT_BLOCK_SPAN() * game.L2_BLOCK_TIME() * 2
        );

        // bad duplicate proposal
        KailuaTournament[PROPOSAL_BUFFER_LEN] memory proposals_256;
        for (uint256 i = 0; i < PROPOSAL_BUFFER_LEN; i++) {
            vm.startPrank(address(bytes20(uint160(10000 * i + 1))));
            proposals_256[i] = treasury.propose(
                Claim.wrap(0x0001010000010100000010100000101000001010000010100000010100000101),
                abi.encodePacked(uint64(256), uint64(proposal_128_0.gameIndex()), uint64(i))
            );
            vm.stopPrank();
        }

        // Generate mock proof
        bytes memory proof = mockFaultProof(
            address(this),
            proposals_128[0].l1Head().raw(),
            anchor.rootClaim().raw(),
            proposal_128_0.rootClaim().raw(),
            uint64(proposals_128[0].l2BlockNumber())
        );

        // Accept fault proof
        anchor.proveOutputFault(
            address(this),
            [uint64(1), uint64(0)],
            proof,
            anchor.rootClaim().raw(),
            KailuaKZGLib.hashToFe(proposals_128[0].rootClaim().raw()),
            proposal_128_0.rootClaim().raw(),
            new bytes[](0),
            new bytes[](0)
        );

        // Prune all contradictions
        //        console2.log("anchor %s:%s/%s", anchor.contenderIndex(), anchor.opponentIndex(), anchor.childCount());
        while (address(anchor.pruneChildren(1)) != address(proposal_128_0)) {
            //            console2.log("anchor %s:%s/%s", anchor.contenderIndex(), anchor.opponentIndex(), anchor.childCount());
        }
        proposal_128_0.resolve();

        //        console2.log(
        //            "pre proposal_128_0 %s:%s/%s",
        //            proposal_128_0.contenderIndex(),
        //            proposal_128_0.opponentIndex(),
        //            proposal_128_0.childCount()
        //        );
        while (proposal_128_0.contenderIndex() < proposal_128_0.childCount()) {
            proposal_128_0.pruneChildren(1);
            //            console2.log(
            //                "zer proposal_128_0 %s:%s/%s",
            //                proposal_128_0.contenderIndex(),
            //                proposal_128_0.opponentIndex(),
            //                proposal_128_0.childCount()
            //            );
        }
        proposal_128_0.pruneChildren(1);
        //        console2.log(
        //            "pos proposal_128_0 %s:%s/%s",
        //            proposal_128_0.contenderIndex(),
        //            proposal_128_0.opponentIndex(),
        //            proposal_128_0.childCount()
        //        );

        // honest proposal
        KailuaTournament proposal_256_X = treasury.propose(
            Claim.wrap(0x0001010000010100000010100000101000001010000010100000010100000101),
            abi.encodePacked(uint64(256), uint64(proposal_128_0.gameIndex()), uint64(PROPOSAL_BUFFER_LEN))
        );

        while (address(proposal_128_0.pruneChildren(1)) != address(proposal_256_X)) {
            //            console2.log(
            //                "nxt proposal_128_0 %s:%s/%s",
            //                proposal_128_0.contenderIndex(),
            //                proposal_128_0.opponentIndex(),
            //                proposal_128_0.childCount()
            //            );
        }
        //        console2.log(
        //            "pos proposal_128_0 %s:%s/%s",
        //            proposal_128_0.contenderIndex(),
        //            proposal_128_0.opponentIndex(),
        //            proposal_128_0.childCount()
        //        );
        vm.warp(
            game.GENESIS_TIME_STAMP() + game.PROPOSAL_TIME_GAP()
                + game.PROPOSAL_OUTPUT_COUNT() * game.OUTPUT_BLOCK_SPAN() * game.L2_BLOCK_TIME() * 3
        );
        proposal_256_X.resolve();
    }

    function test_pruneChildren_opponent() public {
        vm.warp(
            game.GENESIS_TIME_STAMP() + game.PROPOSAL_TIME_GAP()
                + game.PROPOSAL_OUTPUT_COUNT() * game.OUTPUT_BLOCK_SPAN() * game.L2_BLOCK_TIME()
        );
        // honest proposal
        KailuaTournament proposal_128_0 = treasury.propose(
            Claim.wrap(0x0001010000010100000010100000101000001010000010100000010100000100),
            abi.encodePacked(uint64(128), uint64(anchor.gameIndex()), uint64(0))
        );

        // bad proposal
        vm.startPrank(address(0x1));
        KailuaTournament proposal_128_1 = treasury.propose(
            Claim.wrap(0x0001010000010100000010100000101000001010000010100000010100000101),
            abi.encodePacked(uint64(128), uint64(anchor.gameIndex()), uint64(0))
        );
        vm.stopPrank();

        vm.warp(
            game.GENESIS_TIME_STAMP() + game.PROPOSAL_TIME_GAP()
                + game.PROPOSAL_OUTPUT_COUNT() * game.OUTPUT_BLOCK_SPAN() * game.L2_BLOCK_TIME() * 2
        );
        // honest proposal
        KailuaTournament proposal_256_0 = treasury.propose(
            Claim.wrap(0x0001010000010100000010100000101000001010000010100000010100000100),
            abi.encodePacked(uint64(256), uint64(proposal_128_0.gameIndex()), uint64(0))
        );

        // fake hoenst proposal
        vm.startPrank(address(0x1));
        treasury.propose(
            Claim.wrap(0x0001010000010100000010100000101000001010000010100000010100000100),
            abi.encodePacked(uint64(256), uint64(proposal_128_0.gameIndex()), uint64(1))
        );
        vm.stopPrank();

        // Generate mock proof
        bytes memory proof = mockFaultProof(
            address(this),
            proposal_128_1.l1Head().raw(),
            anchor.rootClaim().raw(),
            proposal_128_0.rootClaim().raw(),
            uint64(proposal_128_1.l2BlockNumber())
        );

        // Accept fault proof
        anchor.proveOutputFault(
            address(this),
            [uint64(1), uint64(0)],
            proof,
            anchor.rootClaim().raw(),
            KailuaKZGLib.hashToFe(proposal_128_1.rootClaim().raw()),
            proposal_128_0.rootClaim().raw(),
            new bytes[](0),
            new bytes[](0)
        );

        vm.warp(
            game.GENESIS_TIME_STAMP() + game.PROPOSAL_TIME_GAP()
                + game.PROPOSAL_OUTPUT_COUNT() * game.OUTPUT_BLOCK_SPAN() * game.L2_BLOCK_TIME() * 3
        );
        proposal_128_0.resolve();
        proposal_256_0.resolve();

        // Reject validity proof after resolution
        vm.expectRevert(GameNotInProgress.selector);
        anchor.proveValidity(address(this), uint64(0), proof);
    }
}
