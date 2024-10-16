#!/bin/bash

set -e

cd "$(dirname "$0")"
tailwindcss -c ../tailwind.config.js -i ../templates/tw.css -o ../style.css --watch
