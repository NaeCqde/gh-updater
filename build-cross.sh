#!/bin/sh
# 他OS向けビルド (cross + Docker)。Apple Silicon の Mac で動作確認済みの手順。
# 使い方: ./build-cross.sh   → target/<triple>/release/ に出力
set -e
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER=x86_64-unknown-linux-musl-gcc
export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER=aarch64-unknown-linux-musl-gcc
export CARGO_TARGET_RISCV64GC_UNKNOWN_LINUX_MUSL_LINKER=riscv64-unknown-linux-musl-gcc
# riscv64 の musl は既定で動的リンクになり libgcc_s.so.1 が必要になるため、静的リンクを強制する
export CARGO_TARGET_RISCV64GC_UNKNOWN_LINUX_MUSL_RUSTFLAGS="-C target-feature=+crt-static"
for t in x86_64-unknown-linux-musl aarch64-unknown-linux-musl riscv64gc-unknown-linux-musl; do
  cross build --release --target "$t"
done
# Windows の cross 標準イメージは amd64 のみ → エミュレーションで実行
DOCKER_DEFAULT_PLATFORM=linux/amd64 cross build --release --target x86_64-pc-windows-gnu
