// SPDX-FileCopyrightText: Copyright (c) 2025-2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use blake3::Hasher;

use crate::config::{Installation, Machine, Scenario};

use super::ansible_hash::hash_sources;
use super::layer::{cached_layer, hash_file, hash_files, hash_inputs};
use super::setup::setup;

const INSTALL_CACHE_VERSION: &[u8] = b"tmachine-install-blake3-v1";

pub async fn install(
    machine: &Machine,
    scenario: &Scenario,
    installation: &Installation,
) -> Result<PathBuf> {
    let setup_disk = setup(machine, scenario).await?;
    if installation.playbooks.is_empty() && installation.inputs.is_empty() {
        return Ok(setup_disk);
    }

    let hash = install_hash(&setup_disk, installation)?;
    cached_layer(
        &setup_disk,
        &hash,
        installation.use_galaxy,
        &installation.playbooks,
        &installation.inputs,
    )
    .await
}

fn install_hash(setup_disk: &Path, installation: &Installation) -> Result<String> {
    let mut hasher = Hasher::new();
    hasher.update(INSTALL_CACHE_VERSION);
    hash_file(&mut hasher, setup_disk).context("failed to hash setup disk")?;
    hasher.update(&[u8::from(installation.use_galaxy)]);
    hash_sources(&mut hasher).context("failed to hash Ansible sources")?;
    hash_files(&mut hasher, &installation.playbooks).context("failed to hash install playbooks")?;
    hash_inputs(&mut hasher, &installation.inputs)?;
    Ok(hasher.finalize().to_hex().to_string())
}
