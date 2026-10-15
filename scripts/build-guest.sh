#!/usr/bin/env bash
# Build the v0.3 antumbra_auction guest in RISC Zero's pinned Docker builder and
# copy it to artifacts/programs/v0.3/antumbra_auction.bin. The builder is what
# makes the binary reproducible: the same bytes on a CI runner and on a laptop.
set -euo pipefail
cd "$(dirname "$0")/.."
RISC0_DOCKER_CONTAINER_TAG=r0.1.91.1 cargo risczero build --manifest-path programs/auction/Cargo.toml
cp programs/auction/target/riscv32im-risc0-zkvm-elf/docker/antumbra_auction.bin artifacts/programs/v0.3/antumbra_auction.bin
ls -l artifacts/programs/v0.3/antumbra_auction.bin
