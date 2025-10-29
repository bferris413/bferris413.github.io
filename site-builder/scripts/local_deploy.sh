#!/bin/bash
#
# Deploys changes to site-builder and relevant static files to their local deploy locations.
#
# Those changes are picked up by the already-scheduled cron job.

set -e
set -x

cd "$(dirname "$0")"
pwd
cd ..
pwd
cargo build --release

sudo cp target/release/site-builder /usr/local/bin/site-builder
sudo rm -rf /usr/local/bin/deploy
sudo cp -r -p deploy /usr/local/bin/deploy
