#!/bin/bash

set -e

cd "$(dirname "$0")"
tailwindcss -c ../deploy/tailwind.config.js -i ../deploy/tw.css -o ../site/static/style.css --watch
