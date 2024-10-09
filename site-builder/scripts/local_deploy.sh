#!/bin/bash

set -e

cd "$(dirname "$0")"
cd ..
cargo build --release
sudo mv target/release/site-builder /usr/local/bin/site-builder
