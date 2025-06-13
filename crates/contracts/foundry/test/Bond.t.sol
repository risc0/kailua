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

contract BondTest is KailuaTest {
    KailuaTreasury treasury;
    KailuaGame game;
    KailuaTournament anchor;

    bool reentryFlag;
    bytes reentryData;
    bool reentrySuccess;
    bytes reentryResult;
    bool revertFlag;
    uint256 lastReceived;
    uint256 totalReceived;

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
            uint64(0xA) // 10-second dispute timeout
        );
        // Set collateral requirement
        treasury.setParticipationBond(987);
    }

    function maybeReenter() internal {
        if (reentryFlag) {
            (reentrySuccess, reentryResult) = msg.sender.call(reentryData);
        }
        reentryFlag = false;
    }

    function maybeRevert() internal view {
        if (revertFlag) revert("revert flag");
    }

    function accrue(uint256 value) internal {
        lastReceived = value;
        totalReceived += value;
    }

    fallback() external payable {
        accrue(msg.value);
        maybeReenter();
        maybeRevert();
    }

    receive() external payable {
        accrue(msg.value);
        maybeReenter();
        maybeRevert();
    }

    function test_setParticipationBond() public {
        // Fail to set collateral
        vm.prank(address(0xbeef));
        vm.expectRevert("not owner");
        treasury.setParticipationBond(123);
    }

    function test_claimProposerBond() public {
        // Jump ahead
        vm.warp(
            game.GENESIS_TIME_STAMP() + game.PROPOSAL_OUTPUT_COUNT() * game.OUTPUT_BLOCK_SPAN() * game.L2_BLOCK_TIME()
        );
        // Fail to propose with no collateral
        uint64 anchorIndex = uint64(anchor.gameIndex());
        vm.expectRevert(IncorrectBondAmount.selector);
        treasury.propose(
            Claim.wrap(0x0001010000010100000010100000101000001010000010100000010100000101),
            abi.encodePacked(uint64(128), anchorIndex, uint64(0))
        );
        // Propose with collateral
        KailuaTournament proposal_128_0 = treasury.propose{value: 987}(
            Claim.wrap(0x0001010000010100000010100000101000001010000010100000010100000101),
            abi.encodePacked(uint64(128), anchorIndex, uint64(0))
        );
        // Jump ahead
        vm.warp(
            game.GENESIS_TIME_STAMP()
                + game.PROPOSAL_OUTPUT_COUNT() * game.OUTPUT_BLOCK_SPAN() * game.L2_BLOCK_TIME() * 2
        );
        // Fail to reclaim bond with pending proposal
        vm.expectRevert(GameNotResolved.selector);
        treasury.claimProposerBond();

        // Finalize
        vm.assertEq(treasury.lastResolved(), address(anchor));
        proposal_128_0.resolve();
        vm.assertEq(treasury.lastResolved(), address(proposal_128_0));

        // Fail to receive payment
        revertFlag = true;
        vm.expectRevert(BondTransferFailed.selector);
        treasury.claimProposerBond();
        revertFlag = false;

        // Reclaim bond only once with reentry
        reentryFlag = true;
        reentryData = abi.encodePacked(KailuaTreasury.claimProposerBond.selector);
        treasury.claimProposerBond();
        vm.assertFalse(reentrySuccess);

        // fail to claim money again
        vm.expectRevert(NoCreditToClaim.selector);
        treasury.claimProposerBond();

        // Fail to propose with no collateral
        anchorIndex = uint64(proposal_128_0.gameIndex());
        vm.expectRevert(IncorrectBondAmount.selector);
        treasury.propose(
            Claim.wrap(0x0001010000010100000010100000101000001010000010100000010100000101),
            abi.encodePacked(uint64(256), anchorIndex, uint64(0))
        );
    }

    function test_claimProposerBond_duplicate() public {
        // Jump ahead
        vm.warp(
            game.GENESIS_TIME_STAMP() + game.PROPOSAL_OUTPUT_COUNT() * game.OUTPUT_BLOCK_SPAN() * game.L2_BLOCK_TIME()
        );

        // Propose with collateral
        uint64 anchorIndex = uint64(anchor.gameIndex());
        KailuaTournament proposal_128_0 = treasury.propose{value: 987}(
            Claim.wrap(0x0001010000010100000010100000101000001010000010100000010100000101),
            abi.encodePacked(uint64(128), anchorIndex, uint64(0))
        );

        vm.deal(address(0x01), 1000);
        vm.startPrank(address(0x01));
        treasury.propose{value: 987}(
            Claim.wrap(0x0001010000010100000010100000101000001010000010100000010100000101),
            abi.encodePacked(uint64(128), anchorIndex, uint64(1))
        );
        vm.stopPrank();

        // Jump ahead
        vm.warp(
            game.GENESIS_TIME_STAMP()
                + game.PROPOSAL_OUTPUT_COUNT() * game.OUTPUT_BLOCK_SPAN() * game.L2_BLOCK_TIME() * 2
        );

        // Fail to reclaim bond with pending proposal
        vm.startPrank(address(0x01));
        vm.expectRevert(GameNotResolved.selector);
        treasury.claimProposerBond();
        vm.stopPrank();

        // Finalize
        vm.assertEq(treasury.lastResolved(), address(anchor));
        proposal_128_0.resolve();
        vm.assertEq(treasury.lastResolved(), address(proposal_128_0));

        // Reclaim bond as duplicator
        vm.startPrank(address(0x01));
        treasury.claimProposerBond();
    }

    function test_claimProposerBond_late() public {
        // Jump ahead
        vm.warp(
            game.GENESIS_TIME_STAMP() + game.PROPOSAL_OUTPUT_COUNT() * game.OUTPUT_BLOCK_SPAN() * game.L2_BLOCK_TIME()
        );

        // Propose with collateral
        uint64 anchorIndex = uint64(anchor.gameIndex());
        KailuaTournament proposal_128_0 = treasury.propose{value: 987}(
            Claim.wrap(0x0001010000010100000010100000101000001010000010100000010100000101),
            abi.encodePacked(uint64(128), anchorIndex, uint64(0))
        );

        // Jump ahead
        vm.warp(
            game.GENESIS_TIME_STAMP()
                + game.PROPOSAL_OUTPUT_COUNT() * game.OUTPUT_BLOCK_SPAN() * game.L2_BLOCK_TIME() * 2
        );

        vm.deal(address(0x01), 1000);
        vm.startPrank(address(0x01));
        treasury.propose{value: 987}(
            Claim.wrap(0x0001010000010100000010100000101000001010000010100000010100000100),
            abi.encodePacked(uint64(128), anchorIndex, uint64(0))
        );
        vm.stopPrank();

        // Fail to reclaim bond with pending proposal
        vm.startPrank(address(0x01));
        vm.expectRevert(GameNotResolved.selector);
        treasury.claimProposerBond();
        vm.stopPrank();

        // Finalize
        vm.assertEq(treasury.lastResolved(), address(anchor));
        proposal_128_0.resolve();
        vm.assertEq(treasury.lastResolved(), address(proposal_128_0));

        // Reclaim bond as duplicator
        vm.startPrank(address(0x01));
        treasury.claimProposerBond();
    }

    function test_claimEliminationBond() public {
        // Claim nothing
        for (uint256 i = 0; i < 32; i++) {
            treasury.claimEliminationBonds(i);
            vm.assertEq(treasury.eliminationsPaid(address(this)), 0);
        }

        vm.warp(
            game.GENESIS_TIME_STAMP() + game.PROPOSAL_OUTPUT_COUNT() * game.OUTPUT_BLOCK_SPAN() * game.L2_BLOCK_TIME()
        );
        // Succeed to propose after min creation time
        KailuaTournament proposal_128_0 = treasury.propose{value: 987}(
            Claim.wrap(0x0001010000010100000010100000101000001010000010100000010100000101),
            abi.encodePacked(uint64(128), uint64(anchor.gameIndex()), uint64(0))
        );
        vm.warp(
            game.GENESIS_TIME_STAMP()
                + game.PROPOSAL_OUTPUT_COUNT() * game.OUTPUT_BLOCK_SPAN() * game.L2_BLOCK_TIME() * 2
        );
        // Succeed to propose after min creation time
        KailuaTournament proposal_256_0 = treasury.propose(
            Claim.wrap(0x0001010000010100000010100000101000001010000010100000010100000101),
            abi.encodePacked(uint64(256), uint64(proposal_128_0.gameIndex()), uint64(0))
        );

        // Succeed to eliminate from parent address
        vm.startPrank(address(proposal_128_0));
        treasury.eliminate(address(proposal_256_0), address(this));
        vm.stopPrank();

        // Succeed to claim own elimination bond
        treasury.claimEliminationBonds(1);
        vm.assertEq(lastReceived, 987);
        vm.assertEq(totalReceived, 987);
        vm.assertEq(treasury.paidBonds(address(this)), 0);
        vm.assertEq(treasury.eliminationsPaid(address(this)), 1);

        // Nothing else to claim
        treasury.claimEliminationBonds(100);
        vm.assertEq(totalReceived, 987);
        vm.assertEq(treasury.eliminationsPaid(address(this)), 1);
    }
}
