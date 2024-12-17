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
    KailuaTreasury,
    "foundry/out/KailuaTreasury.sol/KailuaTreasury.json"
);

sol!(
    #[sol(rpc)]
    KailuaTournament,
    "foundry/out/KailuaTournament.sol/KailuaTournament.json"
);

sol!(
    #[sol(rpc)]
    IRiscZeroVerifier,
    "foundry/out/FlatR0ImportV1.2.0.sol/IRiscZeroVerifier.json"
);

sol!(
    #[sol(rpc)]
    RiscZeroVerifierRouter,
    "foundry/out/FlatR0ImportV1.2.0.sol/RiscZeroVerifierRouter.json"
);

sol!(
    #[sol(rpc)]
    RiscZeroSetVerifier,
    "foundry/out/FlatR0ImportV1.2.0.sol/RiscZeroSetVerifier.json"
);

sol!(
    #[sol(rpc)]
    RiscZeroGroth16Verifier,
    "foundry/out/FlatR0ImportV1.2.0.sol/RiscZeroGroth16Verifier.json"
);

sol!(
    #[sol(rpc)]
    RiscZeroMockVerifier,
    "foundry/out/FlatR0ImportV1.2.0.sol/RiscZeroMockVerifier.json"
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
    Safe,
    "foundry/out/FlatOPImportV1.4.0.sol/Safe.json"
);

sol!(
    #[sol(rpc)]
    OptimismPortal2,
    "foundry/out/FlatOPImportV1.4.0.sol/OptimismPortal2.json"
);

sol!(
    #[sol(rpc)]
    SystemConfig,
    "foundry/out/FlatOPImportV1.4.0.sol/SystemConfig.json"
);

sol! {
    #[sol(rpc)]
    struct SetVerifierSeal {
        /// Merkle path to the leaf.
        bytes32[] path;
        /// Root seal.
        bytes rootSeal;
    }
}
