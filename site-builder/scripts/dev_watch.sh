#!/bin/bash

current_dir="$(dirname "$0")"
cd "$current_dir" || exit 1

cargo watch \
    --watch-when-idle \
    -x "run -- --template-file-path=$current_dir/../templates/index.html --out-file-path=$current_dir/../index.html --no-fetch --input-file=$current_dir/../templates/debug_commits.json" \
    -w "$current_dir/../templates/index.html" \
    -w "$current_dir/../src"