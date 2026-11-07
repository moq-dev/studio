#!/usr/bin/env bash
# On nixos2: the instrumented release build both UIs are measured from. Fat LTO
# off: main's two rind versions collide in one LTO module.
set -u
cd "$(dirname "$0")/.."
. .tmp/libs.sh
CARGO_PROFILE_RELEASE_LTO=false CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16 CARGO_TARGET_DIR=.tmp/target \
	nix develop --command sh -c "LIBRARY_PATH=$RUNE_LIBS:\${LIBRARY_PATH:-} quest gate cargo build --release -p moq-studio --features rune,openh264" > .tmp/build.log 2>&1
echo EXIT=$?
grep -E "^error|error\[" .tmp/build.log | head -20
