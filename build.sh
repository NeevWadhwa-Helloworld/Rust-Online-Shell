#!/usr/bin/env bash
set -euo pipefail

echo "==> Installing trunk..."
if ! command -v trunk &>/dev/null; then
    cargo install trunk --locked
fi

echo "==> Adding wasm32 target..."
rustup target add wasm32-unknown-unknown

echo "==> Building Leptos frontend..."
cd frontend
trunk build --release
cd ..

echo "==> Copying dist/ to root..."
cp -r frontend/dist ./dist

echo "==> Building shell binary..."
cargo build --release -p rust_shell

echo "==> Done. Artifacts:"
echo "    dist/            -> Leptos WASM frontend"
echo "    target/release/rust_shell -> Shell binary"
