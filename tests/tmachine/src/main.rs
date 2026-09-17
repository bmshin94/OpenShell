// SPDX-FileCopyrightText: Copyright (c) 2025-2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use config::{Config, Installation, Machine, Scenario};

mod ansible;
mod config;
mod qemu;

#[derive(Parser)]
#[command(name = "tmachine")]
struct Cli {
    #[arg(long, default_value = "config.yaml")]
    config: PathBuf,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Setup {
        scenario: String,
    },
    Install {
        scenario: String,
        installation: String,
    },
    Test {
        scenario: String,
        installation: String,
        testsuite: String,
    },
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let config = Config::load(&cli.config)?;

    match cli.command {
        Command::Setup { scenario } => {
            let (machine, scenario) = find_scenario(&config, &scenario)?;
            qemu::setup(&machine, &scenario).await?;
        }
        Command::Install {
            scenario,
            installation,
        } => {
            let (machine, scenario) = find_scenario(&config, &scenario)?;
            let installation = find_installation(&config, &installation)?;
            qemu::install(&machine, &scenario, &installation).await?;
        }
        Command::Test {
            scenario,
            installation,
            testsuite,
        } => {
            let (machine, scenario) = find_scenario(&config, &scenario)?;
            let installation = find_installation(&config, &installation)?;
            let testsuite = config
                .testsuites
                .iter()
                .find(|candidate| candidate.name == testsuite)
                .with_context(|| format!("testsuite {testsuite:?} is not defined"))?;
            qemu::test(&machine, &scenario, &installation, testsuite).await?;
        }
    }

    Ok(())
}

fn find_installation(config: &Config, name: &str) -> Result<Installation> {
    config
        .installations
        .iter()
        .find(|installation| installation.name == name)
        .with_context(|| format!("installation {name:?} is not defined"))
        .cloned()
}

fn find_scenario(config: &Config, name: &str) -> Result<(Machine, Scenario)> {
    let scenario = config
        .scenarios
        .iter()
        .find(|scenario| scenario.name == name)
        .with_context(|| format!("scenario {name:?} is not defined"))?
        .clone();
    let machine = config
        .machines
        .iter()
        .find(|machine| machine.name == scenario.machine)
        .with_context(|| {
            format!(
                "machine {:?} referenced by scenario {:?} is not defined",
                scenario.machine, scenario.name
            )
        })?
        .clone();

    Ok((machine, scenario))
}
