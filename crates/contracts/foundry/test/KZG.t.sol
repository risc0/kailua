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

contract KZGTest is KailuaTest {
    function setUp() public override {
        super.setUp();
    }

    function verifyKZGBlobProof(uint32 index, uint256 value, bytes calldata commitment, bytes calldata proof)
        external
        returns (bool)
    {
        return
            KailuaKZGLib.verifyKZGBlobProof(KailuaKZGLib.versionedKZGHash(commitment), index, value, commitment, proof);
    }

    function test_verifyKZGBlobProof_0() public {
        bytes memory commitment = abi.encodePacked(
            hex"c00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000"
        );
        bytes memory proof = abi.encodePacked(
            hex"c00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000"
        );
        for (uint32 i = 0; i < 4096; i++) {
            vm.assertTrue(this.verifyKZGBlobProof(i, 0, commitment, proof));
        }
    }

    function test_verifyKZGBlobProof_1() public {
        bytes memory commitment = abi.encodePacked(
            hex"b7f1d3a73197d7942695638c4fa9ac0fc3688c4f9774b905a14e3a3f171bac586c55e83ff97a1aeffb3af00adb22c6bb"
        );
        bytes memory proof = abi.encodePacked(
            hex"c00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000"
        );
        uint256 value = 0x73eda753299d7d483339d80809a1d80553bda402fffe5bfeffffffff00000000;
        for (uint32 i = 0; i < 4096; i++) {
            vm.assertTrue(this.verifyKZGBlobProof(i, value, commitment, proof));
        }
    }

    function test_verifyKZGBlobProof_2() public {
        bytes memory commitment = abi.encodePacked(
            hex"a572cbea904d67468808c8eb50a9450c9721db309128012543902d0ac358a62ae28f75bb8f1c7c42c39a8c5529bf0f4e"
        );
        bytes memory proof = abi.encodePacked(
            hex"c00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000"
        );
        uint256 value = 0x0000000000000000000000000000000000000000000000000000000000000002;
        for (uint32 i = 0; i < 4096; i++) {
            vm.assertTrue(this.verifyKZGBlobProof(i, value, commitment, proof));
        }
    }
}
