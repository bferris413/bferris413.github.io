#!/bin/bash

cd $(dirname $0)

tailwindcss -c ../tailwind.config.js -i ../templates/tw.css -o ../../out/style.css --watch

