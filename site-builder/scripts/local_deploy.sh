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

sudo cp target/release/site-builder /usr/local/bin/site-builder || true
sudo mkdir -p /usr/local/bin/templates

sudo cp templates/index.html /usr/local/bin/templates || true
sudo cp templates/tw.css /usr/local/bin/templates || true
sudo cp tailwind.config.js /usr/local/bin/templates || true
