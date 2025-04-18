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
            uint64(0x0) // no dispute timeout
        );
        // Get anchor proposal
        anchorIndex = uint64(factory.gameCount() - 1);
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
        game_0.resolve();
        game_1.resolve();
        vm.assertEq(treasury.paidBonds(address(this)), treasury.participationBond());
        treasury.claimProposerBond();
        vm.assertEq(treasury.paidBonds(address(this)), 0);
    }

    function test_vanguard() public {
        treasury.assignVanguard(address(0x007), Duration.wrap(0xFFFFFFFFFFFFFFFF));
        vm.assertEq(treasury.vanguard(), address(0x007));

        vm.warp(
            game.GENESIS_TIME_STAMP() + game.PROPOSAL_TIME_GAP()
                + game.PROPOSAL_OUTPUT_COUNT() * game.OUTPUT_BLOCK_SPAN() * game.L2_BLOCK_TIME() * 1
        );
        // Fail if not vanguard
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
        proposal_128_0.resolve();
        vm.expectRevert();
        proposal_128_1.resolve();
    }

    function test_duplication() public {
        vm.warp(
            game.GENESIS_TIME_STAMP() + game.PROPOSAL_TIME_GAP()
                + game.PROPOSAL_OUTPUT_COUNT() * game.OUTPUT_BLOCK_SPAN() * game.L2_BLOCK_TIME() * 1
        );
        // Succeed on fresh proposal
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
        proposal_128_0.resolve();
        vm.expectRevert();
        proposal_128_1.resolve();
    }

    function test_lastProposal() public {
        vm.warp(
            game.GENESIS_TIME_STAMP() + game.PROPOSAL_TIME_GAP()
                + game.PROPOSAL_OUTPUT_COUNT() * game.OUTPUT_BLOCK_SPAN() * game.L2_BLOCK_TIME() * 2
        );
        // Fail on low successor height
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
        proposal_128_0.resolve();
        vm.expectRevert();
        proposal_128_1.resolve();
        proposal_256_0.resolve();
        proposal_384_0.resolve();
        proposal_512_0.resolve();
    }

    function test_minCreationTime() public {
        // Fail before l2 block time
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
        proposal_128_0.resolve();
    }

    function test_extraData() public {
        uint64 parentIndex = anchorIndex;
        uint256 blocksPerProposal = game.PROPOSAL_OUTPUT_COUNT() * game.OUTPUT_BLOCK_SPAN();
        for (uint256 i = 1; i <= 128; i++) {
            vm.warp(game.GENESIS_TIME_STAMP() + game.PROPOSAL_TIME_GAP() + blocksPerProposal * game.L2_BLOCK_TIME() * i);
            KailuaGame proposal = KailuaGame(
                address(
                    treasury.propose(
                        Claim.wrap(sha256(abi.encodePacked(bytes32(i)))),
                        abi.encodePacked(uint64(blocksPerProposal * i), parentIndex, uint64(0))
                    )
                )
            );

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

            parentIndex = uint64(proposal.gameIndex());
        }
    }
}
