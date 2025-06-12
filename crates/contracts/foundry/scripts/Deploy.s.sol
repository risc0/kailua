// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.24;

import {Script} from "forge-std/Script.sol";
import {console} from "forge-std/console.sol";
import "../src/vendor/FlatOPImportV1.4.0.sol";
import "../src/vendor/FlatR0ImportV2.0.2.sol";
import {KailuaTreasury} from "../src/KailuaTreasury.sol";
import {KailuaGame} from "../src/KailuaGame.sol";

// quickly get most of the env variables there
// kailua-cli config --op-node-url $OP_NODE_URL --op-geth-url $OP_GETH_URL --eth-rpc-url $ETH_RPC_URL | grep -E '^[A-Z_]+:' | sed 's/: /=/; s/^/export /' > .env
// source .env

contract DeployScript is Script {
    uint256 deployerPrivateKey = vm.envUint("PRIVATE_KEY");
    address deployer = vm.addr(deployerPrivateKey);

    bytes32 fpvmImageId = vm.envBytes32("FPVM_IMAGE_ID");
    bytes32 controlRoot = vm.envBytes32("CONTROL_ROOT");
    bytes32 controlId = vm.envBytes32("CONTROL_ID");
    IRiscZeroVerifier riscZeroVerifier = IRiscZeroVerifier(vm.envAddress("RISC_ZERO_VERIFIER"));
    bytes32 rollupConfigHash = vm.envBytes32("ROLLUP_CONFIG_HASH");
    uint256 proposalOutputCount = vm.envUint("PROPOSAL_OUTPUT_COUNT");
    uint256 outputBlockSpan = vm.envUint("OUTPUT_BLOCK_SPAN");
    GameType gameType = GameType.wrap(uint32(vm.envUint("KAILUA_GAME_TYPE")));
    IDisputeGameFactory dgf = IDisputeGameFactory(vm.envAddress("DISPUTE_GAME_FACTORY"));
    Claim outputRootClaim = Claim.wrap(vm.envBytes32("OUTPUT_ROOT_CLAIM"));
    uint64 l2BlockNumber = uint64(vm.envUint("L2_BLOCK_NUMBER"));
    uint256 genesisTimestamp = vm.envUint("GENESIS_TIMESTAMP");
    uint256 blocktime = vm.envUint("BLOCK_TIME");
    Duration maxClockDuration = Duration.wrap(uint64(vm.envUint("MAX_CLOCK_DURATION")));
    uint256 participationBond = vm.envUint("PARTICIPATION_BOND");
    address vanguardAddress = vm.envAddress("VANGUARD_ADDRESS");
    Duration vanguardAdvantage = Duration.wrap(uint64(vm.envUint("VANGUARD_ADVANTAGE"))); // set
    OptimismPortal2 optimismPortal = OptimismPortal2(payable(vm.envAddress("OPTIMISM_PORTAL")));

    function run() public {
        vm.startBroadcast(deployerPrivateKey);

        _6_1_proofVerification();
        (KailuaTreasury treasury, KailuaGame game) = _6_2_disputeResolution();
        _6_3_stateAnchoring(treasury);
        _6_4_sequencingProposal(treasury, game);

        vm.stopBroadcast();
    }
    
    function _6_1_proofVerification() public {
        RiscZeroVerifierRouter router = new RiscZeroVerifierRouter(deployer);

        RiscZeroGroth16Verifier groth16Verifier = new RiscZeroGroth16Verifier(controlRoot, controlId);
        bytes4 groth16Selector = groth16Verifier.SELECTOR();
        router.addVerifier(groth16Selector, groth16Verifier);
    }

    function _6_2_disputeResolution() public returns (KailuaTreasury, KailuaGame) {
        KailuaTreasury treasury = new KailuaTreasury(riscZeroVerifier, fpvmImageId, rollupConfigHash, proposalOutputCount, outputBlockSpan, gameType, optimismPortal, outputRootClaim, l2BlockNumber);
        KailuaGame game = new KailuaGame(treasury, genesisTimestamp, blocktime, maxClockDuration);

        return (treasury, game);
    }

    function _6_3_stateAnchoring(KailuaTreasury treasury) public {
        uint256 initialBond = dgf.initBonds(gameType);
        if (initialBond != 0) {
            dgf.setInitBond(gameType, 0);
        }
        dgf.setImplementation(gameType, treasury);
        treasury.propose(outputRootClaim, abi.encodePacked(l2BlockNumber, treasury));
        // Call the games function on the dispute game factory to get the created game
        (IDisputeGame gameAddress,) = dgf.games(gameType, outputRootClaim, abi.encodePacked(l2BlockNumber, treasury));
        gameAddress.resolve();
    }

    function _6_4_sequencingProposal(KailuaTreasury treasury, KailuaGame game) public {
        treasury.setParticipationBond(participationBond);
        dgf.setImplementation(gameType, game);
        // OPTIONAL
        treasury.assignVanguard(vanguardAddress, vanguardAdvantage);
        optimismPortal.setRespectedGameType(gameType);
    }
}
