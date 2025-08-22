// Copyright 2024, 2025 RISC Zero, Inc.
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

import {RollupUserLogic, AssertionInputs, RollupLib} from "./vendor/FlatARBImportV3.1.1.sol";
import "./vendor/FlatOPImportV1.4.0.sol";
import "./KailuaGame.sol";

contract KailuaAnyTrust {
    /// @notice The (expected) KailuaGame contract address
    KailuaGame public immutable KAILUA_GAME;

    /// @notice The Arbitrum Rollup contract address
    RollupUserLogic public immutable ARBITRUM_ROLLUP;

    function fastConfirmResolvedAssertion(
        AssertionInputs calldata assertion,
        bytes32 expectedAssertionHash,
        uint64 gameIndex
    ) external {
        (,, IDisputeGame game) = this.gameAtIndex(gameIndex);
        // resolve game if not resolved
        if (game.status() != GameStatus.DEFENDER_WINS) {
            require(game.resolve() == GameStatus.DEFENDER_WINS, "GAME_NOT_RESOLVED");
        }
        // sanity check assertion hashes
        require(game.rootClaim().raw() == expectedAssertionHash, "EXPECTED_ASSERTION_HASH");
        KailuaTournament parentGame = KailuaTournament(address(game)).parentGame();
        require(
            parentGame.rootClaim().raw()
                == RollupLib.assertionHash(
                    assertion.beforeStateData.prevPrevAssertionHash,
                    assertion.beforeState,
                    assertion.beforeStateData.sequencerBatchAcc
                ),
            "PREVIOUS_ASSERTION_HASH"
        );
        // confirm assertion
        ARBITRUM_ROLLUP.fastConfirmNewAssertion(assertion, expectedAssertionHash);
    }

    // OwnableUpgradeable Emulation
    function owner() public view virtual returns (address) {
        return ARBITRUM_ROLLUP.owner();
    }

    // OptimismPortal emulation
    function disputeGameFactory() public view returns (DisputeGameFactory) {
        return DisputeGameFactory(address(this));
    }

    function respectedGameType() public pure returns (GameType) {
        return GameType.wrap(1337);
    }

    // DisputeGameFactory emulation
    /// @dev Allows for the creation of clone proxies with immutable arguments.
    using LibClone for address;

    /// @notice Mapping of a hash of `gameType || rootClaim || extraData` to the deployed `IDisputeGame` clone (where
    //          `||` denotes concatenation).
    mapping(Hash => GameId) internal _disputeGames;

    /// @notice An append-only array of disputeGames that have been created. Used by offchain game solvers to
    ///         efficiently track dispute games.
    GameId[] internal _disputeGameList;

    function getGameUUID(GameType _gameType, Claim _rootClaim, bytes calldata _extraData)
        public
        pure
        returns (Hash uuid_)
    {
        uuid_ = Hash.wrap(keccak256(abi.encode(_gameType, _rootClaim, _extraData)));
    }

    function create(GameType _gameType, Claim _rootClaim, bytes calldata _extraData)
        external
        payable
        returns (IDisputeGame proxy_)
    {
        // Ensure gameType is correct
        require(_gameType.raw() == KAILUA_GAME.gameType().raw(), "GAME_TYPE");

        // Get the hash of the parent block.
        bytes32 parentHash = blockhash(block.number - 1);

        // Grab the implementation contract
        address impl = _disputeGameList.length == 0 ? address(KAILUA_GAME.KAILUA_TREASURY()) : address(KAILUA_GAME);

        // Clone the implementation contract and initialize it with the given parameters.
        //
        // CWIA Calldata Layout:
        // ┌──────────────┬────────────────────────────────────┐
        // │    Bytes     │            Description             │
        // ├──────────────┼────────────────────────────────────┤
        // │ [0, 20)      │ Game creator address               │
        // │ [20, 52)     │ Root claim                         │
        // │ [52, 84)     │ Parent block hash at creation time │
        // │ [84, 84 + n) │ Extra data (opaque)                │
        // └──────────────┴────────────────────────────────────┘
        proxy_ = IDisputeGame(impl.clone(abi.encodePacked(msg.sender, _rootClaim, parentHash, _extraData)));
        proxy_.initialize();

        // Compute the unique identifier for the dispute game.
        Hash uuid = getGameUUID(_gameType, _rootClaim, _extraData);

        // If a dispute game with the same UUID already exists, revert.
        if (GameId.unwrap(_disputeGames[uuid]) != bytes32(0)) revert GameAlreadyExists(uuid);

        // Pack the game ID.
        GameId id = LibGameId.pack(_gameType, Timestamp.wrap(uint64(block.timestamp)), address(proxy_));

        // Store the dispute game id in the mapping & emit the `DisputeGameCreated` event.
        _disputeGames[uuid] = id;
        _disputeGameList.push(id);
        emit IDisputeGameFactory.DisputeGameCreated(address(proxy_), _gameType, _rootClaim);
    }

    function gameAtIndex(uint256 _index)
        external
        view
        returns (GameType gameType_, Timestamp timestamp_, IDisputeGame proxy_)
    {
        (GameType gameType, Timestamp timestamp, address proxy) = _disputeGameList[_index].unpack();
        (gameType_, timestamp_, proxy_) = (gameType, timestamp, IDisputeGame(proxy));
    }

    function games(GameType _gameType, Claim _rootClaim, bytes calldata _extraData)
        external
        view
        returns (IDisputeGame proxy_, Timestamp timestamp_)
    {
        Hash uuid = getGameUUID(_gameType, _rootClaim, _extraData);
        (, Timestamp timestamp, address proxy) = _disputeGames[uuid].unpack();
        (proxy_, timestamp_) = (IDisputeGame(proxy), timestamp);
    }

    function gameCount() external view returns (uint256 gameCount_) {
        gameCount_ = _disputeGameList.length;
    }
}
