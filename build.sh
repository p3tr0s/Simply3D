#!/bin/sh
# Simply3D - build and run on Linux (needs Rust from https://rustup.rs; see README for the
# system libraries)
set -e
cd "$(dirname "$0")"
if ! command -v cargo >/dev/null 2>&1; then
    echo "Rust is not installed. Get it from https://rustup.rs and run this again."
    exit 1
fi
cargo build --release
echo
echo "Built: $(pwd)/target/release/simply3d"
exec ./target/release/simply3d "$@"
