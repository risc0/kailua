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
pragma solidity ^0.8.15;

import "./vendor/FlatOPImportV1.4.0.sol";
import "./vendor/FlatR0ImportV1.2.0.sol";
import "./KailuaLib.sol";
import "./KailuaTournament.sol";

contract KailuaTreasury is KailuaTournament, IKailuaTreasury {
    /// @notice Semantic version.
    /// @custom:semver 0.1.0
    string public constant version = "0.1.0";

    // ------------------------------
    // Immutable configuration
    // ------------------------------

    constructor(
        IRiscZeroVerifier _verifierContract,
        bytes32 _imageId,
        bytes32 _configHash,
        uint256 _proposalBlockCount,
        GameType _gameType,
        IDisputeGameFactory _disputeGameFactory
    )
        KailuaTournament(
            KailuaTreasury(this),
            _verifierContract,
            _imageId,
            _configHash,
            _proposalBlockCount,
            _gameType,
            _disputeGameFactory
        )
    {
        proposerOf[address(this)] = address(this);
    }

    // ------------------------------
    // IInitializable implementation
    // ------------------------------

    /// @inheritdoc IInitializable
    function initialize() external payable override {
        super.initializeInternal();

        OwnableUpgradeable factoryContract = OwnableUpgradeable(address(DISPUTE_GAME_FACTORY));
        if (gameCreator() != factoryContract.owner()) {
            revert BadAuth();
        }
    }

    // ------------------------------
    // IDisputeGame implementation
    // ------------------------------

    /// @inheritdoc IDisputeGame
    function extraData() external pure returns (bytes memory extraData_) {
        // The extra data starts at the second word within the cwia calldata and
        // is 32 bytes long.
        extraData_ = _getArgBytes(0x54, 0x08);
    }

    /// @inheritdoc IDisputeGame
    function resolve() external onlyFactoryOwner returns (GameStatus status_) {
        // INVARIANT: Resolution cannot occur unless the game is currently in progress.
        if (status != GameStatus.IN_PROGRESS) {
            revert GameNotInProgress();
        }

        // Update the status and emit the resolved event, note that we're performing a storage update here.
        emit Resolved(status = status_ = GameStatus.DEFENDER_WINS);

        // Mark resolution timestamp
        resolvedAt = Timestamp.wrap(uint64(block.timestamp));
    }

    // ------------------------------
    // Fault proving
    // ------------------------------

    /// @inheritdoc KailuaTournament
    function verifyIntermediateOutput(uint64, bytes32, bytes calldata, bytes calldata)
        external
        pure
        override
        returns (bool success)
    {
        success = false;
    }

    /// @inheritdoc KailuaTournament
    function getChallengerDuration(uint256) public pure override returns (Duration duration_) {
        duration_ = Duration.wrap(0);
    }

    /// @inheritdoc KailuaTournament
    function parentGame() public view override returns (KailuaTournament parentGame_) {
        parentGame_ = this;
    }

    // ------------------------------
    // IKailuaTreasury implementation
    // ------------------------------

    /// @inheritdoc IKailuaTreasury
    mapping(address => uint256) public eliminationRound;

    /// @inheritdoc IKailuaTreasury
    mapping(address => address) public proposerOf;

    /// @inheritdoc IKailuaTreasury
    function eliminate(address _child, address prover) external {
        KailuaTournament child = KailuaTournament(_child);

        // INVARIANT: Only the child's parent may call this
        KailuaTournament parent = child.parentGame();
        if (msg.sender != address(parent)) {
            revert Blacklisted(msg.sender, address(parent));
        }

        // INVARIANT: Only known proposals may be eliminated
        address eliminated = proposerOf[address(child)];
        if (eliminated == address(0x0)) {
            revert NotProposed();
        }

        // INVARIANT: Cannot double-eliminate players
        if (eliminationRound[eliminated] > 0) {
            revert AlreadyEliminated();
        }

        // Record elimination round
        eliminationRound[eliminated] = child.gameIndex();

        // Transfer bond payment to the game's prover
        pay(paidBonds[eliminated], prover);
    }

    // ------------------------------
    // Treasury
    // ------------------------------

    uint256 public participationBond;

    mapping(address => uint256) public paidBonds;

    modifier onlyFactoryOwner() {
        OwnableUpgradeable factoryContract = OwnableUpgradeable(address(DISPUTE_GAME_FACTORY));
        require(msg.sender == factoryContract.owner(), "Ownable: caller is not the owner");
        _;
    }

    /// @notice Transfers ETH from the contract's balance to the recipient
    function pay(uint256 amount, address recipient) internal {
        (bool success,) = recipient.call{value: amount}(hex"");
        if (!success) revert BondTransferFailed();
    }

    /// @notice Updates the required bond for new proposals
    function setParticipationBond(uint256 amount) external onlyFactoryOwner {
        participationBond = amount;
        emit BondUpdated(amount);
    }

    bool public isProposing;

    /// @notice Checks the proposer's bonded amount and creates a new proposal through the factory
    function propose(Claim _rootClaim, bytes calldata _extraData)
        external
        payable
        returns (KailuaTournament gameContract)
    {
        // Check proposer honesty
        if (eliminationRound[msg.sender] > 0) {
            revert BadAuth();
        }
        // Update proposer bond
        if (msg.value > 0) {
            paidBonds[msg.sender] += msg.value;
        }
        // Check proposer bond
        if (paidBonds[msg.sender] < participationBond) {
            revert IncorrectBondAmount();
        }
        // Create proposal
        isProposing = true;
        gameContract = KailuaTournament(address(DISPUTE_GAME_FACTORY.create(GAME_TYPE, _rootClaim, _extraData)));
        isProposing = false;
        // Record proposer
        proposerOf[address(gameContract)] = msg.sender;
    }
}
