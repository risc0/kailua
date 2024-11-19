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

#![allow(clippy::too_many_arguments)]
use alloy::sol;

sol!(
    #[sol(rpc)]
    KailuaGame,
    "foundry/out/KailuaGame.sol/KailuaGame.json"
);

sol!(
    #[sol(rpc)]
    KailuaSetup,
    "foundry/out/KailuaSetup.sol/KailuaSetup.json"
);

sol!(
    #[sol(rpc)]
    KailuaTournament,
    "foundry/out/KailuaTournament.sol/KailuaTournament.json"
);

sol!(
    #[sol(rpc)]
    RiscZeroMockVerifier,
    "foundry/out/MockVerifier.sol/RiscZeroMockVerifier.json"
);

sol!(
    #[sol(rpc)]
    OwnableUpgradeable,
    "foundry/out/FlatOPImportV1.4.0.sol/OwnableUpgradeable.json"
);

sol!(
    #[sol(rpc)]
    IDisputeGameFactory,
    "foundry/out/FlatOPImportV1.4.0.sol/IDisputeGameFactory.json"
);

sol!(
    #[sol(rpc)]
    IAnchorStateRegistry,
    "foundry/out/FlatOPImportV1.4.0.sol/IAnchorStateRegistry.json"
);

sol!(
    #[sol(rpc)]
    Safe,
    "foundry/out/FlatOPImportV1.4.0.sol/Safe.json"
);

sol! {
    #[sol(rpc)]
    interface OptimismPortal {
        function setRespectedGameType(uint32 _gameType) external;
        function guardian() public view returns (address);
    }
}
