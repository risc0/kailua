// SPDX-License-Identifier: UNLICENSED
pragma solidity ^0.8.10;

import "./KailuaTest.sol";

contract Propose is KailuaTest {
    KailuaTreasury treasury;
    KailuaGame game;
    uint64 anchorIndex;

    function setUp() public override {
        super.setUp();
        // Deploy a Blobless system
        (treasury, game) = deployKailua(
            uint256(0x1),
            uint256(0x80),
            sha256(abi.encodePacked(bytes32(0x00))),
            uint64(0x0),
            uint256(0x0),
            uint256(0x0),
            uint256(0x0),
            uint64(0x0)
        );
        // Get anchor proposal
        anchorIndex = uint64(factory.gameCount() - 1);
    }

    fallback() external payable {}

    receive() external payable {}

    function test_participationBond() public {
        treasury.setParticipationBond(123);
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
        // Fail if not vanguard
        vm.expectRevert();
        treasury.propose(
            Claim.wrap(0x0001010000010100000010100000101000001010000010100000010100000101),
            abi.encodePacked(uint64(128), anchorIndex, uint64(0))
        );
        // Success with vanguard
        vm.prank(treasury.vanguard());
        treasury.propose(
            Claim.wrap(0x0001010000010100000010100000101000001010000010100000010100000101),
            abi.encodePacked(uint64(128), anchorIndex, uint64(0))
        );
        // Success after vanguard
        treasury.propose(
            Claim.wrap(0x000101000001010000001010000010100000101000001010000001010000010F),
            abi.encodePacked(uint64(128), anchorIndex, uint64(0))
        );
    }

    function test_duplication() public {
        // Succeed on fresh proposal
        KailuaTournament proposal = treasury.propose(
            Claim.wrap(0x0001010000010100000010100000101000001010000010100000010100000101),
            abi.encodePacked(uint64(128), anchorIndex, uint64(0))
        );
        // Fail on duplicate with same counter
        vm.startPrank(address(0x007));
        uint256 gameIndex = proposal.gameIndex();
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
        treasury.propose(
            Claim.wrap(0x0001010000010100000010100000101000001010000010100000010100000101),
            abi.encodePacked(uint64(128), anchorIndex, uint64(1))
        );
        vm.stopPrank();
    }

    function test_progression() public {
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
        KailuaTournament proposal = treasury.propose(
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
        treasury.propose(
            Claim.wrap(0x000101000001010000001010000010100000101000001010000001010000010F),
            abi.encodePacked(uint64(128), anchorIndex, uint64(0))
        );
        // Succeed on successor proposal
        // [128, 256]
        // [128]
        proposal = treasury.propose(
            Claim.wrap(0x0001010000010100000010100000101000001010000010100000010100000101),
            abi.encodePacked(uint64(256), uint64(proposal.gameIndex()), uint64(0))
        );
        uint256 proposal_256_index = proposal.gameIndex();
        // Succeed on successor proposal
        // [128, 256, 384]
        // [128]
        proposal = treasury.propose(
            Claim.wrap(0x0001010000010100000010100000101000001010000010100000010100000101),
            abi.encodePacked(uint64(384), uint64(proposal.gameIndex()), uint64(0))
        );
        // Succeed on child proposal for new proposer
        // [128, 256, 384]
        // [128, 512]
        vm.startPrank(address(0x007));
        proposal = treasury.propose(
            Claim.wrap(0x000101000001010000001010000010100000101000001010000001010000010F),
            abi.encodePacked(uint64(512), uint64(proposal.gameIndex()), uint64(0))
        );
        // Fail on out of order proposal for new proposer
        vm.startPrank(address(0x007));
        vm.expectRevert();
        treasury.propose(
            Claim.wrap(0x000101000001010000001010000010100000101000001010000001010000010F),
            abi.encodePacked(uint64(512), uint64(proposal_256_index), uint64(0))
        );
    }
}
