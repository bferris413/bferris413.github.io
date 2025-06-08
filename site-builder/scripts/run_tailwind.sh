#!/bin/bash

set -e

cd "$(dirname "$0")"/../deploy
echo "running from $(pwd)"

tailwindcss -c tailwind.config.js -i tw.css -o ../site/static/style.css --watch
