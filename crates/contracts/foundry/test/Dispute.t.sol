// SPDX-License-Identifier: UNLICENSED
pragma solidity ^0.8.10;

import "./KailuaTest.sol";

contract Propose is KailuaTest {
    KailuaTreasury treasury;
    KailuaGame game;
    uint64 anchorIndex;

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

        // Fail to resolve disputed claims
        for (uint256 i = 1; i < 12; i++) {
            vm.expectRevert();
            proposal_128[i].resolve();
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

        // Fail to finalize disproven claim
        vm.expectRevert();
        proposal_128_0.resolve();
    }

    function test_proveOutputFault_disputed() public {
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

        // Fail to resolve without dispute resolution
        vm.warp(block.timestamp + game.MAX_CLOCK_DURATION().raw());
        vm.expectRevert();
        proposal_128_0.resolve();
        for (uint256 i = 1; i < 12; i++) {
            vm.expectRevert();
            proposal_128[i].resolve();
        }

        // Submit fault proofs
        for (uint256 i = 1; i < 12; i++) {
            // Generate mock proof
            bytes memory proof = mockFaultProof(
                address(this),
                proposal_128[i].l1Head().raw(),
                proposal_128[i].parentGame().rootClaim().raw(),
                proposal_128_0.rootClaim().raw(),
                uint64(proposal_128[i].l2BlockNumber())
            );

            // Accept fault proof
            proposal_128_0.parentGame().proveOutputFault(
                address(this),
                [uint64(i), uint64(0)],
                proof,
                proposal_128[i].parentGame().rootClaim().raw(),
                KailuaKZGLib.hashToFe(proposal_128[i].rootClaim().raw()),
                proposal_128_0.rootClaim().raw(),
                new bytes[](0),
                new bytes[](0)
            );
        }

        // Finalize
        proposal_128_0.resolve();
    }
}
