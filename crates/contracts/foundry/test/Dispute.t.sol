// SPDX-License-Identifier: UNLICENSED
pragma solidity ^0.8.10;

import "./KailuaTest.sol";

contract Propose is KailuaTest {
    KailuaTreasury treasury;
    KailuaGame game;
    uint64 anchorIndex;

    uint256 public constant PROPOSAL_BUFFER_LEN = 21;

    function setUp() public override {
        super.setUp();
        // Deploy dispute contracts
        (treasury, game) = deployKailua(
            uint256(0x1), // no intermediate commitments
            uint256(0x80), // 128 blocks per proposal
            sha256(abi.encodePacked(bytes32(0x00))), // arbitrary block hash
            uint64(0x0), // genesis
            uint256(block.timestamp), // start l2 from now
            uint256(0x1), // 1-second block times
            uint256(0x5), // 5-second wait
            uint64(0xA) // 10-second dispute timeout
        );
        // Get anchor proposal
        anchorIndex = uint64(factory.gameCount() - 1);
    }

    function test_getChallengerDuration() public {
        vm.warp(
            game.GENESIS_TIME_STAMP() + game.PROPOSAL_TIME_GAP()
                + game.PROPOSAL_OUTPUT_COUNT() * game.OUTPUT_BLOCK_SPAN() * game.L2_BLOCK_TIME()
        );
        // Succeed to propose after proposal time gap
        KailuaTournament proposal_128_0 = treasury.propose(
            Claim.wrap(0x0001010000010100000010100000101000001010000010100000010100000101),
            abi.encodePacked(uint64(128), anchorIndex, uint64(0))
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

    function test_proveValidity_undisputed() public {
        vm.warp(
            game.GENESIS_TIME_STAMP() + game.PROPOSAL_TIME_GAP()
                + game.PROPOSAL_OUTPUT_COUNT() * game.OUTPUT_BLOCK_SPAN() * game.L2_BLOCK_TIME()
        );
        // Succeed to propose after proposal time gap
        KailuaTournament proposal_128_0 = treasury.propose(
            Claim.wrap(0x0001010000010100000010100000101000001010000010100000010100000101),
            abi.encodePacked(uint64(128), anchorIndex, uint64(0))
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

    function test_proveValidity_disputed() public {
        vm.warp(
            game.GENESIS_TIME_STAMP() + game.PROPOSAL_TIME_GAP()
                + game.PROPOSAL_OUTPUT_COUNT() * game.OUTPUT_BLOCK_SPAN() * game.L2_BLOCK_TIME()
        );
        // Succeed to propose after proposal time gap
        KailuaTournament proposal_128_0 = treasury.propose(
            Claim.wrap(0x0001010000010100000010100000101000001010000010100000010100000101),
            abi.encodePacked(uint64(128), anchorIndex, uint64(0))
        );

        KailuaTournament[12] memory proposal_128;
        for (uint256 i = 1; i < 12; i++) {
            vm.startPrank(address(bytes20(uint160(i))));
            proposal_128[i] = treasury.propose(
                Claim.wrap(sha256(abi.encodePacked(bytes32(i)))), abi.encodePacked(uint64(128), anchorIndex, uint64(0))
            );
            vm.stopPrank();
        }

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

        // Fail to resolve without dispute resolution
        vm.warp(block.timestamp + game.MAX_CLOCK_DURATION().raw());
        vm.expectRevert();
        proposal_128_0.resolve();
        for (uint256 i = 1; i < 12; i++) {
            vm.expectRevert();
            proposal_128[i].resolve();
        }

        // Accept validity proof
        proposal_128_0.parentGame().proveValidity(address(this), uint64(0), proof);

        for (uint256 i = 1; i < 12; i++) {
            // Fail to resolve disputed claims
            vm.expectRevert();
            proposal_128[i].resolve();
            // Ensure signature is unviable
            vm.assertFalse(proposal_128[i].parentGame().isViableSignature(proposal_128[i].signature()));
        }

        // Finalize
        proposal_128_0.resolve();
    }

    function test_proveOutputFault_undisputed() public {
        vm.warp(
            game.GENESIS_TIME_STAMP() + game.PROPOSAL_TIME_GAP()
                + game.PROPOSAL_OUTPUT_COUNT() * game.OUTPUT_BLOCK_SPAN() * game.L2_BLOCK_TIME()
        );
        // Succeed to propose after proposal time gap
        KailuaTournament proposal_128_0 = treasury.propose(
            Claim.wrap(0x0001010000010100000010100000101000001010000010100000010100000101),
            abi.encodePacked(uint64(128), anchorIndex, uint64(0))
        );

        // Generate mock proof
        bytes memory proof = mockFaultProof(
            address(this),
            proposal_128_0.l1Head().raw(),
            proposal_128_0.parentGame().rootClaim().raw(),
            ~proposal_128_0.rootClaim().raw(),
            uint64(proposal_128_0.l2BlockNumber())
        );

        // Accept fault proof
        proposal_128_0.parentGame().proveOutputFault(
            address(this),
            [uint64(0), uint64(0)],
            proof,
            proposal_128_0.parentGame().rootClaim().raw(),
            KailuaKZGLib.hashToFe(proposal_128_0.rootClaim().raw()),
            ~proposal_128_0.rootClaim().raw(),
            new bytes[](0),
            new bytes[](0)
        );

        // Ensure signature is unviable
        vm.assertFalse(proposal_128_0.parentGame().isViableSignature(proposal_128_0.signature()));

        // Fail to finalize disproven claim
        vm.expectRevert();
        proposal_128_0.resolve();
    }

    function test_proveOutputFault_disputed() public {
        uint64 parentIndex = anchorIndex;

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
        uint64 parentIndex = anchorIndex;

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
}
