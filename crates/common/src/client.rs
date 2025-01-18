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

use crate::blobs::{hash_to_fe, PreloadedBlobProvider};
use crate::journal::ProofJournal;
use crate::precondition::PreconditionValidationData;
use crate::witness::Witness;
use alloy_eips::eip4844::{Blob, FIELD_ELEMENTS_PER_BLOB};
use alloy_primitives::{Address, Sealed, B256};
use anyhow::{bail, Context};
use kona_derive::traits::BlobProvider;
use kona_driver::Driver;
use kona_executor::TrieDBProvider;
use kona_preimage::{CommsClient, PreimageKey, PreimageKeyType};
use kona_proof::errors::OracleProviderError;
use kona_proof::executor::KonaExecutor;
use kona_proof::l1::{OracleL1ChainProvider, OraclePipeline};
use kona_proof::l2::OracleL2ChainProvider;
use kona_proof::sync::new_pipeline_cursor;
use kona_proof::{BootInfo, FlushableCache, HintType};
use maili_genesis::RollupConfig;
use risc0_zkvm::sha::{Impl as SHA2, Sha256};
use std::cmp::Ordering;
use std::fmt::Debug;
use std::sync::Arc;

pub fn run_client<
    O: CommsClient + FlushableCache + Send + Sync + Debug,
    B: BlobProvider + Send + Sync + Debug + Clone,
>(
    precondition_validation_data_hash: B256,
    oracle: Arc<O>,
    mut beacon: B,
) -> anyhow::Result<(Arc<BootInfo>, B256, Option<B256>)>
where
    <B as BlobProvider>::Error: Debug,
{
    kona_proof::block_on(async move {
        ////////////////////////////////////////////////////////////////
        //                          PROLOGUE                          //
        ////////////////////////////////////////////////////////////////
        log("BOOT");
        let boot = Arc::new(
            BootInfo::load(oracle.as_ref())
                .await
                .context("BootInfo::load")?,
        );

        log("PRECONDITION");
        let precondition_data = load_precondition_data(
            precondition_validation_data_hash,
            oracle.clone(),
            &mut beacon,
        )
        .await?;

        let safe_head_hash = fetch_safe_head_hash(oracle.as_ref(), boot.as_ref()).await?;

        let mut l1_provider = OracleL1ChainProvider::new(boot.l1_head, oracle.clone());
        let mut l2_provider =
            OracleL2ChainProvider::new(safe_head_hash, boot.rollup_config.clone(), oracle.clone());

        // If the claimed L2 block number is less than or equal to the safe head of the L2 chain,
        // the claim is invalid.
        // Fetch the safe head's block header.
        let safe_head = l2_provider
            .header_by_hash(safe_head_hash)
            .map(|header| Sealed::new_unchecked(header, safe_head_hash))?;

        if boot.claimed_l2_block_number < safe_head.number {
            bail!("Invalid claim");
        }
        let safe_head_number = safe_head.number;

        ////////////////////////////////////////////////////////////////
        //                   DERIVATION & EXECUTION                   //
        ////////////////////////////////////////////////////////////////
        log("DERIVATION & EXECUTION");
        // Create a new derivation driver with the given boot information and oracle.
        let cursor = new_pipeline_cursor(
            &boot.rollup_config,
            safe_head,
            &mut l1_provider,
            &mut l2_provider,
        )
        .await?;
        l2_provider.set_cursor(cursor.clone());

        let cfg = Arc::new(boot.rollup_config.clone());
        let pipeline = OraclePipeline::new(
            cfg.clone(),
            cursor.clone(),
            oracle.clone(),
            beacon,
            l1_provider.clone(),
            l2_provider.clone(),
        );
        let executor =
            KonaExecutor::new(&cfg, l2_provider.clone(), l2_provider.clone(), None, None);
        let mut driver = Driver::new(cursor, executor, pipeline);

        // Run the derivation pipeline until we are able to produce the output root of the claimed
        // L2 block.
        let expected_output_count = (boot.claimed_l2_block_number - safe_head_number) as usize;
        let mut output_roots = Vec::with_capacity(expected_output_count);
        for starting_block in safe_head_number..boot.claimed_l2_block_number {
            // Advance to the next target
            let (output_number, _, output_root) = driver
                .advance_to_target(&boot.rollup_config, Some(starting_block + 1))
                .await?;
            // Stop if nothing new was derived
            if output_number == starting_block {
                // A mismatch indicates that there is insufficient L1 data available to produce
                // an L2 output root at the claimed block number
                log(&format!(
                    "OUTPUT: {output_number}|{}",
                    boot.claimed_l2_block_number
                ));
                break;
            }
            // Append newly computed output root
            output_roots.push(output_root);
        }

        ////////////////////////////////////////////////////////////////
        //                          EPILOGUE                          //
        ////////////////////////////////////////////////////////////////
        log("EPILOGUE");

        let precondition_hash = precondition_data
            .map(|(precondition_validation_data, blobs)| {
                validate_precondition(
                    precondition_validation_data,
                    blobs,
                    safe_head_number,
                    &output_roots,
                )
            })
            .unwrap_or(Ok(B256::ZERO))?;

        if output_roots.len() != expected_output_count {
            // Not enough data to derive output root at claimed height
            Ok((boot, precondition_hash, None))
        } else if output_roots.is_empty() {
            // Claimed output height is equal to agreed output height
            let real_output_hash = boot.agreed_l2_output_root;
            Ok((boot, precondition_hash, Some(real_output_hash)))
        } else {
            // Derived output root at future height
            Ok((boot, precondition_hash, output_roots.pop()))
        }
    })
}

/// Fetches the safe head hash of the L2 chain based on the agreed upon L2 output root in the
/// [BootInfo].
pub async fn fetch_safe_head_hash<O>(
    caching_oracle: &O,
    boot_info: &BootInfo,
) -> Result<B256, OracleProviderError>
where
    O: CommsClient,
{
    let mut output_preimage = [0u8; 128];
    HintType::StartingL2Output
        .get_exact_preimage(
            caching_oracle,
            boot_info.agreed_l2_output_root,
            PreimageKeyType::Keccak256,
            &mut output_preimage,
        )
        .await?;

    output_preimage[96..128]
        .try_into()
        .map_err(OracleProviderError::SliceConversion)
}

pub fn log(msg: &str) {
    #[cfg(target_os = "zkvm")]
    risc0_zkvm::guest::env::log(msg);
    #[cfg(not(target_os = "zkvm"))]
    tracing::info!("{msg}");
}

fn safe_default<V: Debug + Eq>(opt: Option<V>, default: V) -> anyhow::Result<V> {
    if let Some(v) = opt {
        if v == default {
            anyhow::bail!(format!("Unsafe value! {v:?}"))
        }
        Ok(v)
    } else {
        Ok(default)
    }
}

pub fn config_hash(rollup_config: &RollupConfig) -> anyhow::Result<[u8; 32]> {
    // todo: check whether we need to include this, or if it is loaded from the config address
    let system_config_hash: [u8; 32] = rollup_config
        .genesis
        .system_config
        .as_ref()
        .map(|system_config| {
            let fields = [
                system_config.batcher_address.0.as_slice(),
                system_config.overhead.to_be_bytes::<32>().as_slice(),
                system_config.scalar.to_be_bytes::<32>().as_slice(),
                system_config.gas_limit.to_be_bytes().as_slice(),
                safe_default(system_config.base_fee_scalar, u64::MAX)
                    .context("base_fee_scalar")?
                    .to_be_bytes()
                    .as_slice(),
                safe_default(system_config.blob_base_fee_scalar, u64::MAX)
                    .context("blob_base_fee_scalar")?
                    .to_be_bytes()
                    .as_slice(),
            ]
            .concat();
            let digest = SHA2::hash_bytes(fields.as_slice());

            Ok::<[u8; 32], anyhow::Error>(digest.as_bytes().try_into()?)
        })
        .unwrap_or(Ok([0u8; 32]))?;
    let rollup_config_bytes = [
        rollup_config.genesis.l1.hash.0.as_slice(),
        rollup_config.genesis.l2.hash.0.as_slice(),
        system_config_hash.as_slice(),
        rollup_config.block_time.to_be_bytes().as_slice(),
        rollup_config.max_sequencer_drift.to_be_bytes().as_slice(),
        rollup_config.seq_window_size.to_be_bytes().as_slice(),
        rollup_config.channel_timeout.to_be_bytes().as_slice(),
        rollup_config
            .granite_channel_timeout
            .to_be_bytes()
            .as_slice(),
        rollup_config.l1_chain_id.to_be_bytes().as_slice(),
        rollup_config.l2_chain_id.to_be_bytes().as_slice(),
        rollup_config
            .base_fee_params
            .max_change_denominator
            .to_be_bytes()
            .as_slice(),
        rollup_config
            .base_fee_params
            .elasticity_multiplier
            .to_be_bytes()
            .as_slice(),
        rollup_config
            .canyon_base_fee_params
            .max_change_denominator
            .to_be_bytes()
            .as_slice(),
        rollup_config
            .canyon_base_fee_params
            .elasticity_multiplier
            .to_be_bytes()
            .as_slice(),
        safe_default(rollup_config.regolith_time, u64::MAX)
            .context("regolith_time")?
            .to_be_bytes()
            .as_slice(),
        safe_default(rollup_config.canyon_time, u64::MAX)
            .context("canyon_time")?
            .to_be_bytes()
            .as_slice(),
        safe_default(rollup_config.delta_time, u64::MAX)
            .context("delta_time")?
            .to_be_bytes()
            .as_slice(),
        safe_default(rollup_config.ecotone_time, u64::MAX)
            .context("ecotone_time")?
            .to_be_bytes()
            .as_slice(),
        safe_default(rollup_config.fjord_time, u64::MAX)
            .context("fjord_time")?
            .to_be_bytes()
            .as_slice(),
        safe_default(rollup_config.granite_time, u64::MAX)
            .context("granite_time")?
            .to_be_bytes()
            .as_slice(),
        safe_default(rollup_config.holocene_time, u64::MAX)
            .context("holocene_time")?
            .to_be_bytes()
            .as_slice(),
        safe_default(rollup_config.blobs_enabled_l1_timestamp, u64::MAX)
            .context("blobs_enabled_timestmap")?
            .to_be_bytes()
            .as_slice(),
        rollup_config.batch_inbox_address.0.as_slice(),
        rollup_config.deposit_contract_address.0.as_slice(),
        rollup_config.l1_system_config_address.0.as_slice(),
        rollup_config.protocol_versions_address.0.as_slice(),
        safe_default(rollup_config.superchain_config_address, Address::ZERO)
            .context("superchain_config_address")?
            .0
            .as_slice(),
        safe_default(rollup_config.da_challenge_address, Address::ZERO)
            .context("da_challenge_address")?
            .0
            .as_slice(),
    ]
    .concat();
    let digest = SHA2::hash_bytes(rollup_config_bytes.as_slice());
    Ok::<[u8; 32], anyhow::Error>(digest.as_bytes().try_into()?)
}

pub async fn load_precondition_data<
    O: CommsClient + Send + Sync + Debug,
    B: BlobProvider + Send + Sync + Debug + Clone,
>(
    precondition_data_hash: B256,
    oracle: Arc<O>,
    beacon: &mut B,
) -> anyhow::Result<Option<(PreconditionValidationData, Vec<Blob>)>>
where
    <B as BlobProvider>::Error: Debug,
{
    if precondition_data_hash.is_zero() {
        return Ok(None);
    }
    // Read the blob references to fetch
    let precondition_validation_data: PreconditionValidationData = pot::from_slice(
        &oracle
            .get(PreimageKey::new(
                *precondition_data_hash,
                PreimageKeyType::Sha256,
            ))
            .await
            .map_err(OracleProviderError::Preimage)?,
    )?;
    let mut blobs = Vec::new();
    // Read the blobs to validate divergence
    for request in precondition_validation_data.blob_fetch_requests() {
        blobs.push(
            *beacon
                .get_blobs(&request.block_ref, &[request.blob_hash.clone()])
                .await
                .unwrap()[0],
        );
    }

    Ok(Some((precondition_validation_data, blobs)))
}

pub fn validate_precondition(
    precondition_validation_data: PreconditionValidationData,
    blobs: Vec<Blob>,
    local_l2_head_number: u64,
    output_roots: &[B256],
) -> anyhow::Result<B256> {
    let precondition_hash = precondition_validation_data.precondition_hash();
    match precondition_validation_data {
        PreconditionValidationData::Fault(divergence_index, _) => {
            // Check equivalence of two blobs until potential divergence point
            if divergence_index == 0 {
                bail!("Unexpected divergence index 0");
            } else if divergence_index > FIELD_ELEMENTS_PER_BLOB {
                bail!(
                    "Divergence index value {divergence_index} exceeds {FIELD_ELEMENTS_PER_BLOB}"
                );
            }
            // Check for equality
            for i in 0..divergence_index {
                let index = 32 * i as usize;
                if blobs[0][index..index + 32] != blobs[1][index..index + 32] {
                    bail!("Elements at equivalence position {i} in blobs are not equal.");
                }
            }
            // Check for inequality
            let index = 32 * divergence_index as usize;
            if blobs[0][index..index + 32] == blobs[1][index..index + 32] {
                bail!("Elements at divergence position {divergence_index} in blobs are equal.");
            }
        }
        PreconditionValidationData::Validity(
            global_l2_head_number,
            proposal_output_count,
            output_block_span,
            _,
        ) => {
            // Ensure local and global block ranges match
            if global_l2_head_number > local_l2_head_number {
                bail!(
                    "Validity precondition global starting block #{} > local agreed l2 head #{}",
                    global_l2_head_number,
                    local_l2_head_number
                )
            } else if output_roots.is_empty() {
                bail!("No output roots to check for validity precondition.")
            }
            // Calculate blob index pointer
            let max_block_number =
                global_l2_head_number + proposal_output_count * output_block_span;
            for (i, output_hash) in output_roots.iter().enumerate() {
                let output_block_number = local_l2_head_number + i as u64 + 1;
                if output_block_number > max_block_number {
                    // We should not derive outputs beyond the proposal root claim
                    bail!("Output block #{output_block_number} > max block #{max_block_number}.");
                } else if output_block_number % output_block_span != 0 {
                    // We only check equivalence every output_block_span blocks
                    continue;
                }
                let output_offset =
                    ((output_block_number - global_l2_head_number) / output_block_span) - 1;
                let blob_index = (output_offset / FIELD_ELEMENTS_PER_BLOB) as usize;
                let fe_position = (output_offset % FIELD_ELEMENTS_PER_BLOB) as usize;
                let blob_fe_index = 32 * fe_position;
                // Verify fe equivalence to computed outputs for all but last output
                match output_offset.cmp(&(proposal_output_count - 1)) {
                    Ordering::Less => {
                        // verify equivalence to blob
                        let blob_fe_slice = &blobs[blob_index][blob_fe_index..blob_fe_index + 32];
                        let output_fe = hash_to_fe(*output_hash);
                        let output_fe_bytes = output_fe.to_be_bytes::<32>();
                        if blob_fe_slice != output_fe_bytes.as_slice() {
                            bail!(
                                "Bad fe #{} in blob {} for block #{}: Expected {} found {} ",
                                fe_position,
                                blob_index,
                                output_block_number,
                                B256::try_from(output_fe_bytes.as_slice())?,
                                B256::try_from(blob_fe_slice)?
                            );
                        }
                    }
                    Ordering::Equal => {
                        // verify zeroed trail data
                        if blob_index != blobs.len() - 1 {
                            bail!(
                                "Expected trail data to begin at blob {blob_index}/{}",
                                blobs.len()
                            );
                        } else if blobs[blob_index][blob_fe_index..].iter().any(|b| b != &0u8) {
                            bail!("Found non-zero trail data in blob {blob_index}");
                        }
                    }
                    Ordering::Greater => {
                        // (output_block_number <= max_block_number) implies:
                        // (output_offset <= proposal_output_count)
                        unreachable!(
                            "Output offset {output_offset} > output count {proposal_output_count}."
                        );
                    }
                }
            }
        }
    }
    // Return the precondition hash
    Ok(precondition_hash)
}

pub fn run_in_memory_client(witness: Witness) -> ProofJournal {
    log(&format!(
        "ORACLE: {} PREIMAGES",
        witness.oracle_witness.preimages.len()
    ));
    witness.oracle_witness.validate().expect("Failed to validate preimages");
    let oracle = Arc::new(witness.oracle_witness);
    log(&format!(
        "BEACON: {} BLOBS",
        witness.blobs_witness.blobs.len()
    ));
    let beacon = PreloadedBlobProvider::from(witness.blobs_witness);

    // Attempt to recompute the output hash at the target block number using kona
    log("RUN");
    let (boot, precondition_hash, computed_output_opt) = crate::client::run_client(
        witness.precondition_validation_data_hash,
        oracle.clone(),
        beacon,
    )
    .expect("Failed to compute output hash.");

    // Validate the output root
    if let Some(computed_output) = computed_output_opt {
        // With sufficient data, the input l2_claim must be true
        assert_eq!(boot.claimed_l2_output_root, computed_output);
    } else {
        // We use the zero claim hash to denote that the data as of l1 head is insufficient
        assert_eq!(boot.claimed_l2_output_root, B256::ZERO);
    }

    // Stitch boots together into a journal
    let mut stitched_journal = ProofJournal::new(
        witness.fpvm_image_id,
        witness.payout_recipient_address,
        precondition_hash,
        boot.as_ref(),
    );
    for stitched_boot in witness.stitched_boot_info {
        // Require equivalence in reference head
        assert_eq!(stitched_boot.l1_head, stitched_journal.l1_head);
        // Require progress in stitched boot
        assert_ne!(
            stitched_boot.agreed_l2_output_root,
            stitched_boot.claimed_l2_output_root
        );
        // Require proof assumption
        #[cfg(target_os = "zkvm")]
        risc0_zkvm::guest::env::verify(
            witness.fpvm_image_id.0,
            &ProofJournal::new_stitched(
                witness.fpvm_image_id,
                witness.payout_recipient_address,
                precondition_hash,
                stitched_journal.config_hash,
                &stitched_boot,
            )
            .encode_packed(),
        )
        .expect("Failed to verify stitched boot assumption");
        // Require continuity
        if stitched_boot.claimed_l2_output_root == stitched_journal.agreed_l2_output_root {
            // Backward stitch
            stitched_journal.agreed_l2_output_root = stitched_boot.agreed_l2_output_root;
        } else if stitched_boot.agreed_l2_output_root == stitched_journal.claimed_l2_output_root {
            // Forward stitch
            stitched_journal.claimed_l2_output_root = stitched_boot.claimed_l2_output_root;
            stitched_journal.claimed_l2_block_number = stitched_boot.claimed_l2_block_number;
        } else {
            unimplemented!("No support for non-contiguous stitching.");
        }
    }

    stitched_journal
}
