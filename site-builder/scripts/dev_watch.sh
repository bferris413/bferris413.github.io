#!/bin/bash

current_dir="$(dirname "$0")"
cd "$current_dir" || exit 1
current_dir="$(pwd)"
export GH_TOKEN=$(cat ../../.token)
export IP_API_TOKEN=$(cat ../../.ip_token)

cargo watch \
    --watch-when-idle \
    -s "cd $current_dir/../deploy && tailwindcss -c ./tailwind.config.js -i ./tw.css -o ../site/static/style.css" \
    -x "run -- --project-dir="$current_dir/../deploy/" --out-dir=$current_dir/../site" \
    -w "$current_dir/../deploy" \
    -w "$current_dir/../src"

# cargo watch \
#     --watch-when-idle \
#     -x "run -- --template-file-path=$current_dir/../templates/index.html --out-file-path=$current_dir/../index.html --no-fetch --input-file=$current_dir/../templates/debug_commits.json" \
#     -w "$current_dir/../templates/index.html" \
#     -w "$current_dir/../src"
