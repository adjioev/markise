#!/bin/sh
# PROTOTYPE (proto/block-forms): builds dist/form.js (JSON Forms page of the block edit dialog).
# The bundle is committed so Markise builds without Node; rebuild after changing src/ or deps:
#   cd apps/markise-web-render/form && sh build.sh
set -e
cd "$(dirname "$0")"
[ -d node_modules ] || npm ci --no-audit --no-fund
npx esbuild src/main.jsx --bundle --minify --format=iife --target=safari15 \
  --define:process.env.NODE_ENV=\"production\" --legal-comments=eof --outfile=dist/form.js
ls -l dist/form.js
