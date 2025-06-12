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

contract ProposeTest is KailuaTest {
    KailuaTreasury treasury;
    KailuaGame game;
    KailuaTournament anchor;

    function setUp() public override {
        super.setUp();
        // Deploy dispute contracts
        (treasury, game, anchor) = deployKailua(
            uint64(0x1), // no intermediate commitments
            uint64(0x80), // 128 blocks per proposal
            sha256(abi.encodePacked(bytes32(0x00))), // arbitrary block hash
            uint64(0x0), // genesis
            uint256(block.timestamp), // start l2 from now
            uint256(0x1), // 1-second block times
            uint256(0x5), // 5-second wait
            uint64(0x0) // no dispute timeout
        );
    }

    fallback() external payable {}

    receive() external payable {}

    function test_participationBond() public {
        treasury.setParticipationBond(123);
        vm.warp(
            game.GENESIS_TIME_STAMP() + game.PROPOSAL_TIME_GAP()
                + game.PROPOSAL_OUTPUT_COUNT() * game.OUTPUT_BLOCK_SPAN() * game.L2_BLOCK_TIME() * 1
        );
        // Fail without deposit
        uint64 anchorIndex = uint64(anchor.gameIndex());
        vm.expectRevert(IncorrectBondAmount.selector);
        treasury.propose(
            Claim.wrap(0x0001010000010100000010100000101000001010000010100000010100000101),
            abi.encodePacked(uint64(128), anchorIndex, uint64(0))
        );
        // Success with collateral
        KailuaTournament game_0 = treasury.propose{value: treasury.participationBond()}(
            Claim.wrap(0x0001010000010100000010100000101000001010000010100000010100000101),
            abi.encodePacked(uint64(128), uint64(factory.gameCount() - 1), uint64(0))
        );

        vm.warp(
            game.GENESIS_TIME_STAMP() + game.PROPOSAL_TIME_GAP()
                + game.PROPOSAL_OUTPUT_COUNT() * game.OUTPUT_BLOCK_SPAN() * game.L2_BLOCK_TIME() * 2
        );
        // Success without more collateral
        KailuaTournament game_1 = treasury.propose(
            Claim.wrap(0x0001010000010100000010100000101000001010000010100000010100000102),
            abi.encodePacked(uint64(256), uint64(factory.gameCount() - 1), uint64(0))
        );
        // Withdraw collateral
        vm.assertEq(treasury.lastResolved(), address(anchor));
        game_0.resolve();
        vm.assertEq(treasury.lastResolved(), address(game_0));
        game_1.resolve();
        vm.assertEq(treasury.lastResolved(), address(game_1));
        vm.assertEq(treasury.paidBonds(address(this)), treasury.participationBond());
        treasury.claimProposerBond();
        vm.assertEq(treasury.paidBonds(address(this)), 0);
    }

    function test_vanguard() public {
        // Fail assignment
        vm.prank(address(0xbeef));
        vm.expectRevert("not owner");
        treasury.assignVanguard(address(0x007), Duration.wrap(0xFFFFFFFFFFFFFFFF));
        vm.assertEq(treasury.vanguard(), address(0x0));

        // Assign vanguard
        treasury.assignVanguard(address(0x007), Duration.wrap(0xFFFFFFFFFFFFFFFF));
        vm.assertEq(treasury.vanguard(), address(0x007));

        vm.warp(
            game.GENESIS_TIME_STAMP() + game.PROPOSAL_TIME_GAP()
                + game.PROPOSAL_OUTPUT_COUNT() * game.OUTPUT_BLOCK_SPAN() * game.L2_BLOCK_TIME() * 1
        );
        // Fail if not vanguard
        uint64 anchorIndex = uint64(anchor.gameIndex());
        vm.expectRevert();
        treasury.propose(
            Claim.wrap(0x0001010000010100000010100000101000001010000010100000010100000101),
            abi.encodePacked(uint64(128), anchorIndex, uint64(0))
        );
        // Success with vanguard
        vm.prank(treasury.vanguard());
        KailuaTournament proposal_128_0 = treasury.propose(
            Claim.wrap(0x0001010000010100000010100000101000001010000010100000010100000101),
            abi.encodePacked(uint64(128), anchorIndex, uint64(0))
        );
        // Success after vanguard
        KailuaTournament proposal_128_1 = treasury.propose(
            Claim.wrap(0x000101000001010000001010000010100000101000001010000001010000010F),
            abi.encodePacked(uint64(128), anchorIndex, uint64(0))
        );
        // Finalize
        vm.assertEq(treasury.lastResolved(), address(anchor));
        proposal_128_0.resolve();
        vm.assertEq(treasury.lastResolved(), address(proposal_128_0));
        vm.expectRevert();
        proposal_128_1.resolve();
        vm.assertEq(treasury.lastResolved(), address(proposal_128_0));
    }

    function test_duplication() public {
        vm.warp(
            game.GENESIS_TIME_STAMP() + game.PROPOSAL_TIME_GAP()
                + game.PROPOSAL_OUTPUT_COUNT() * game.OUTPUT_BLOCK_SPAN() * game.L2_BLOCK_TIME() * 1
        );
        // Succeed on fresh proposal
        uint64 anchorIndex = uint64(anchor.gameIndex());
        KailuaTournament proposal_128_0 = treasury.propose(
            Claim.wrap(0x0001010000010100000010100000101000001010000010100000010100000101),
            abi.encodePacked(uint64(128), anchorIndex, uint64(0))
        );
        // Fail on duplicate with same counter
        vm.startPrank(address(0x007));
        vm.expectRevert();
        treasury.propose(
            Claim.wrap(0x0001010000010100000010100000101000001010000010100000010100000101),
            abi.encodePacked(uint64(128), anchorIndex, uint64(0))
        );
        // Fail on higher than expected duplicate counter
        vm.expectRevert();
        treasury.propose(
            Claim.wrap(0x0001010000010100000010100000101000001010000010100000010100000101),
            abi.encodePacked(uint64(128), anchorIndex, uint64(2))
        );
        // Succeed on correct next counter
        KailuaTournament proposal_128_1 = treasury.propose(
            Claim.wrap(0x0001010000010100000010100000101000001010000010100000010100000101),
            abi.encodePacked(uint64(128), anchorIndex, uint64(1))
        );
        vm.stopPrank();
        // Finalize
        vm.assertEq(treasury.lastResolved(), address(anchor));
        proposal_128_0.resolve();
        vm.assertEq(treasury.lastResolved(), address(proposal_128_0));
        vm.expectRevert();
        proposal_128_1.resolve();
        vm.assertEq(treasury.lastResolved(), address(proposal_128_0));
    }

    function test_appendChild() public {
        vm.warp(
            game.GENESIS_TIME_STAMP() + game.PROPOSAL_TIME_GAP()
                + game.PROPOSAL_OUTPUT_COUNT() * game.OUTPUT_BLOCK_SPAN() * game.L2_BLOCK_TIME() * 2
        );
        // Succeed on fresh proposal
        uint64 anchorIndex = uint64(anchor.gameIndex());
        KailuaTournament proposal_128_0 = treasury.propose(
            Claim.wrap(0x0001010000010100000010100000101000001010000010100000010100000101),
            abi.encodePacked(uint64(128), anchorIndex, uint64(0))
        );

        // Fail to call append child outside treasury
        vm.expectRevert(UnknownGame.selector);
        proposal_128_0.appendChild();

        // Fail to append child after resolution
        vm.assertEq(treasury.lastResolved(), address(anchor));
        proposal_128_0.resolve();
        vm.assertEq(treasury.lastResolved(), address(proposal_128_0));
        vm.startPrank(address(0x0));
        vm.expectRevert(ClaimAlreadyResolved.selector);
        treasury.propose(
            Claim.wrap(0x0001010000010100000010100000101000001010000010100000010100000101),
            abi.encodePacked(uint64(128), anchorIndex, uint64(1))
        );
        vm.stopPrank();
    }

    function test_proposerOf() public {
        vm.warp(
            game.GENESIS_TIME_STAMP() + game.PROPOSAL_TIME_GAP()
                + game.PROPOSAL_OUTPUT_COUNT() * game.OUTPUT_BLOCK_SPAN() * game.L2_BLOCK_TIME() * 2
        );

        // Succeed on first proposal from 0x0000..
        vm.startPrank(address(0x0));
        KailuaTournament proposal_128_0 = treasury.propose(
            Claim.wrap(0x0001010000010100000010100000101000001010000010100000010100000101),
            abi.encodePacked(uint64(128), uint64(anchor.gameIndex()), uint64(0))
        );
        vm.stopPrank();

        // Fail on creating a child
        uint64 parentIndex = uint64(proposal_128_0.gameIndex());
        vm.expectRevert(InvalidParent.selector);
        treasury.propose(
            Claim.wrap(0x0001010000010100000010100000101000001010000010100000010100000101),
            abi.encodePacked(uint64(256), parentIndex, uint64(0))
        );

        // Fail to re-init
        vm.expectRevert(AlreadyInitialized.selector);
        proposal_128_0.initialize();

        // Fail to finalize
        vm.assertEq(treasury.lastResolved(), address(anchor));
        vm.expectRevert(NotProposed.selector);
        proposal_128_0.resolve();
        vm.assertEq(treasury.lastResolved(), address(anchor));
    }

    function test_nullClaim() public {
        vm.warp(
            game.GENESIS_TIME_STAMP() + game.PROPOSAL_TIME_GAP()
                + game.PROPOSAL_OUTPUT_COUNT() * game.OUTPUT_BLOCK_SPAN() * game.L2_BLOCK_TIME() * 2
        );

        // Fail to propose 0x0000..
        uint64 parentIndex = uint64(anchor.gameIndex());
        vm.expectPartialRevert(UnexpectedRootClaim.selector);
        treasury.propose(Claim.wrap(bytes32(0x0)), abi.encodePacked(uint64(128), parentIndex, uint64(0)));
    }

    function test_selfParent() public {
        vm.warp(
            game.GENESIS_TIME_STAMP() + game.PROPOSAL_TIME_GAP()
                + game.PROPOSAL_OUTPUT_COUNT() * game.OUTPUT_BLOCK_SPAN() * game.L2_BLOCK_TIME() * 2
        );

        // Fail to extend self as parent
        vm.mockCall(address(treasury), 0, abi.encodePacked(), abi.encodePacked(uint256(1)));
        vm.startPrank(address(anchor));
        vm.expectRevert(InvalidParent.selector);
        anchor.appendChild();
        vm.stopPrank();
        vm.clearMockedCalls();

        // Fail to propose with self as parent
        uint64 parentIndex = uint64(factory.gameCount());
        vm.expectRevert();
        treasury.propose(
            Claim.wrap(0x0001010000010100000010100000101000001010000010100000010100000101),
            abi.encodePacked(uint64(128), parentIndex, uint64(0))
        );
    }

    function test_gameCreator() public {
        vm.warp(
            game.GENESIS_TIME_STAMP() + game.PROPOSAL_TIME_GAP()
                + game.PROPOSAL_OUTPUT_COUNT() * game.OUTPUT_BLOCK_SPAN() * game.L2_BLOCK_TIME()
        );

        // Fail to bypass treasury.propose
        GameType gameType = treasury.GAME_TYPE();
        uint64 parentIndex = uint64(anchor.gameIndex());
        vm.expectPartialRevert(Blacklisted.selector);
        factory.create(
            gameType,
            Claim.wrap(0x0001010000010100000010100000101000001010000010100000010100000101),
            abi.encodePacked(uint64(256), parentIndex, uint64(0))
        );
    }

    function test_lastProposal() public {
        vm.warp(
            game.GENESIS_TIME_STAMP() + game.PROPOSAL_TIME_GAP()
                + game.PROPOSAL_OUTPUT_COUNT() * game.OUTPUT_BLOCK_SPAN() * game.L2_BLOCK_TIME() * 2
        );
        // Fail on low successor height
        uint64 anchorIndex = uint64(anchor.gameIndex());
        vm.expectRevert();
        treasury.propose(
            Claim.wrap(0x0001010000010100000010100000101000001010000010100000010100000101),
            abi.encodePacked(uint64(64), anchorIndex, uint64(0))
        );
        // Fail on high successor height
        vm.expectRevert();
        treasury.propose(
            Claim.wrap(0x0001010000010100000010100000101000001010000010100000010100000101),
            abi.encodePacked(uint64(256), anchorIndex, uint64(0))
        );
        // Succeed on correct successor height
        // [128]
        KailuaTournament proposal_128_0 = treasury.propose(
            Claim.wrap(0x0001010000010100000010100000101000001010000010100000010100000101),
            abi.encodePacked(uint64(128), anchorIndex, uint64(0))
        );
        // Fail on out of order proposal
        vm.expectRevert();
        treasury.propose(
            Claim.wrap(0x0001010000010100000010100000101000001010000010100000010100000101),
            abi.encodePacked(uint64(128), anchorIndex, uint64(0))
        );
        // Succeed on sibling proposal for new proposer
        vm.prank(address(0x007));
        // [128]
        // [128]
        KailuaTournament proposal_128_1 = treasury.propose(
            Claim.wrap(0x000101000001010000001010000010100000101000001010000001010000010F),
            abi.encodePacked(uint64(128), anchorIndex, uint64(0))
        );
        // Succeed on successor proposal
        // [128, 256]
        // [128]
        KailuaTournament proposal_256_0 = treasury.propose(
            Claim.wrap(0x0001010000010100000010100000101000001010000010100000010100000101),
            abi.encodePacked(uint64(256), uint64(proposal_128_0.gameIndex()), uint64(0))
        );

        vm.warp(
            game.GENESIS_TIME_STAMP() + game.PROPOSAL_TIME_GAP()
                + game.PROPOSAL_OUTPUT_COUNT() * game.OUTPUT_BLOCK_SPAN() * game.L2_BLOCK_TIME() * 3
        );
        // Succeed on successor proposal
        // [128, 256, 384]
        // [128]
        KailuaTournament proposal_384_0 = treasury.propose(
            Claim.wrap(0x0001010000010100000010100000101000001010000010100000010100000101),
            abi.encodePacked(uint64(384), uint64(proposal_256_0.gameIndex()), uint64(0))
        );

        vm.warp(
            game.GENESIS_TIME_STAMP() + game.PROPOSAL_TIME_GAP()
                + game.PROPOSAL_OUTPUT_COUNT() * game.OUTPUT_BLOCK_SPAN() * game.L2_BLOCK_TIME() * 4
        );
        // Succeed on child proposal for new proposer
        // [128, 256, 384]
        // [128, 512]
        vm.startPrank(address(0x007));
        KailuaTournament proposal_512_0 = treasury.propose(
            Claim.wrap(0x000101000001010000001010000010100000101000001010000001010000010F),
            abi.encodePacked(uint64(512), uint64(proposal_384_0.gameIndex()), uint64(0))
        );
        // Fail on out of order proposal for new proposer
        uint256 proposal_256_index = proposal_256_0.gameIndex();
        vm.expectRevert();
        treasury.propose(
            Claim.wrap(0x000101000001010000001010000010100000101000001010000001010000010F),
            abi.encodePacked(uint64(384), uint64(proposal_256_index), uint64(0))
        );
        vm.stopPrank();
        // Finalize
        vm.assertEq(treasury.lastResolved(), address(anchor));
        proposal_128_0.resolve();
        vm.assertEq(treasury.lastResolved(), address(proposal_128_0));
        vm.expectRevert();
        proposal_128_1.resolve();
        vm.assertEq(treasury.lastResolved(), address(proposal_128_0));
        proposal_256_0.resolve();
        vm.assertEq(treasury.lastResolved(), address(proposal_256_0));
        proposal_384_0.resolve();
        vm.assertEq(treasury.lastResolved(), address(proposal_384_0));
        proposal_512_0.resolve();
        vm.assertEq(treasury.lastResolved(), address(proposal_512_0));
    }

    function test_minCreationTime() public {
        // Fail before l2 block time
        uint64 anchorIndex = uint64(anchor.gameIndex());
        vm.expectRevert();
        treasury.propose(
            Claim.wrap(0x0001010000010100000010100000101000001010000010100000010100000101),
            abi.encodePacked(uint64(128), anchorIndex, uint64(0))
        );

        vm.warp(
            game.GENESIS_TIME_STAMP() + game.PROPOSAL_OUTPUT_COUNT() * game.OUTPUT_BLOCK_SPAN() * game.L2_BLOCK_TIME()
        );
        // Fail before proposal time gap
        vm.expectRevert();
        treasury.propose(
            Claim.wrap(0x0001010000010100000010100000101000001010000010100000010100000101),
            abi.encodePacked(uint64(128), anchorIndex, uint64(0))
        );

        vm.warp(
            game.GENESIS_TIME_STAMP() + game.PROPOSAL_TIME_GAP()
                + game.PROPOSAL_OUTPUT_COUNT() * game.OUTPUT_BLOCK_SPAN() * game.L2_BLOCK_TIME()
        );
        // Succeed after proposal time gap
        KailuaTournament proposal_128_0 = treasury.propose(
            Claim.wrap(0x0001010000010100000010100000101000001010000010100000010100000101),
            abi.encodePacked(uint64(128), anchorIndex, uint64(0))
        );

        // Validate expected min creation time
        vm.assertEq(proposal_128_0.minCreationTime().raw(), uint64(block.timestamp));

        // Finalize
        vm.assertEq(treasury.lastResolved(), address(anchor));
        proposal_128_0.resolve();
        vm.assertEq(treasury.lastResolved(), address(proposal_128_0));
    }

    function test_KailuaTreasury_extraData() public {
        bytes32 rootClaim = treasury.rootClaim().raw();
        uint64 l2BlockNumber = uint64(treasury.l2BlockNumber());
        // Kailua
        KailuaTreasury new_treasury = new KailuaTreasury(
            verifier,
            bytes32(0x0),
            bytes32(0x0),
            treasury.PROPOSAL_OUTPUT_COUNT(),
            treasury.OUTPUT_BLOCK_SPAN(),
            GameType.wrap(1337),
            OptimismPortal2(payable(address(portal))),
            Claim.wrap(rootClaim),
            uint64(treasury.l2BlockNumber())
        );
        // Anchoring
        factory.setImplementation(GameType.wrap(1337), IDisputeGame(address(new_treasury)));

        // fail to propose with bad root claim
        vm.expectPartialRevert(UnexpectedRootClaim.selector);
        new_treasury.propose(Claim.wrap(~rootClaim), abi.encodePacked(l2BlockNumber, address(new_treasury)));

        // fail to propose with bad extra data
        vm.expectRevert(BadExtraData.selector);
        new_treasury.propose(Claim.wrap(rootClaim), abi.encodePacked(uint256(l2BlockNumber), address(new_treasury)));

        // fail to propose with bad extra data
        vm.expectRevert(BadExtraData.selector);
        new_treasury.propose(Claim.wrap(rootClaim), abi.encodePacked(l2BlockNumber, address(treasury)));

        // fail to propose with bad root claim
        vm.expectPartialRevert(BlockNumberMismatch.selector);
        new_treasury.propose(Claim.wrap(rootClaim), abi.encodePacked(l2BlockNumber + 1, address(new_treasury)));
    }

    function test_KailuaTreasury_resolve() public {
        // Fail to resolve anonymously
        vm.startPrank(address(0xdeadbeef));
        vm.expectRevert("not owner");
        treasury.resolve();
        vm.stopPrank();
        // Fail to resolve again
        vm.expectRevert(GameNotInProgress.selector);
        anchor.resolve();
    }

    function test_KailuaTreasury_verifyIntermediateOutput() public {
        vm.assertFalse(treasury.verifyIntermediateOutput(0, 0, hex"", hex""));
        vm.assertFalse(anchor.verifyIntermediateOutput(0, 0, hex"", hex""));
    }

    function test_KailuaTreasury_getChallengerDuration() public view {
        vm.assertEq(anchor.getChallengerDuration(anchor.createdAt().raw()).raw(), 0);
    }

    function test_extraData() public {
        uint64 parentIndex = uint64(anchor.gameIndex());
        uint256 blocksPerProposal = game.PROPOSAL_OUTPUT_COUNT() * game.OUTPUT_BLOCK_SPAN();
        // Propose in order
        for (uint256 i = 1; i <= 128; i++) {
            vm.warp(game.GENESIS_TIME_STAMP() + game.PROPOSAL_TIME_GAP() + blocksPerProposal * game.L2_BLOCK_TIME() * i);
            bytes32 claim = sha256(abi.encodePacked(bytes32(i)));
            // Fail with bad extra data
            vm.expectRevert(BadExtraData.selector);
            treasury.propose(Claim.wrap(claim), abi.encodePacked(uint64(blocksPerProposal * i), parentIndex, uint72(0)));

            // Succeed with correct info
            KailuaGame proposal = KailuaGame(
                address(
                    treasury.propose(
                        Claim.wrap(claim), abi.encodePacked(uint64(blocksPerProposal * i), parentIndex, uint64(0))
                    )
                )
            );

            // Verify initialization data
            vm.assertEq(
                proposal.extraData(),
                abi.encodePacked(
                    uint64(proposal.l2BlockNumber()),
                    uint64(proposal.parentGameIndex()),
                    uint64(proposal.duplicationCounter())
                )
            );
            vm.assertEq(proposal.l2BlockNumber(), blocksPerProposal * i);
            vm.assertEq(proposal.parentGameIndex(), uint256(parentIndex));
            vm.assertEq(proposal.duplicationCounter(), 0);

            // Verify other data
            vm.assertEq(proposal.proposer(), address(this));
            vm.assertEq(proposal.gameType().raw(), treasury.GAME_TYPE().raw());
            vm.assertEq(proposal.gameCreator(), address(treasury));
            (GameType gameType_, Claim rootClaim_, bytes memory extraData_) = proposal.gameData();
            vm.assertEq(proposal.gameType().raw(), gameType_.raw());
            vm.assertEq(proposal.rootClaim().raw(), rootClaim_.raw());
            vm.assertEq(proposal.extraData(), extraData_);

            parentIndex = uint64(proposal.gameIndex());
        }
        vm.assertEq(treasury.lastResolved(), address(anchor));
        // Fail to resolve out of order
        (,, IDisputeGame lastGame) = factory.gameAtIndex(parentIndex);
        for (
            KailuaTournament proposal = KailuaTournament(address(lastGame));
            proposal.parentGame() != anchor;
            proposal = proposal.parentGame()
        ) {
            vm.expectRevert(OutOfOrderResolution.selector);
            proposal.resolve();
            KailuaTournament parent = proposal.parentGame();
            vm.expectRevert(GameNotResolved.selector);
            parent.pruneChildren(128);
        }
        // Resolve in order
        for (
            KailuaTournament proposal = anchor.pruneChildren(128);
            proposal.childCount() > 0;
            proposal = proposal.pruneChildren(128)
        ) {
            proposal.parentGame().pruneChildren(128);
            proposal.resolve();
            vm.assertEq(treasury.lastResolved(), address(proposal));
        }
        // Resolve last game
        lastGame.resolve();
        vm.assertEq(treasury.lastResolved(), address(lastGame));
        // Test nothing to prune
        vm.expectRevert(NotProposed.selector);
        KailuaTournament(address(lastGame)).pruneChildren(128);
    }
}
