#!/bin/bash
# Builds the llama-server the package installs, for a Raspberry Pi 5 on
# Debian 12. Run by .github/workflows/llama-server.yml in a debian:12
# container; runs the same way on the Pi. Writes, in the output folder:
# llama-server-<tag>-debian12-arm64, its .sha256 and llama.cpp's LICENSE.
#
# usage: build/llama-server.sh <output folder>
set -euo pipefail

tag=b12223
sha256=92210b1f75abd45e9122f5d7d7a6afd32fbfa663d7e200cf2ef576758cd5ef8a
out=$(realpath "$1")
name="llama-server-$tag-debian12-arm64"

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
curl --fail --silent --show-error --location --output "$work/src.tar.gz" \
    "https://github.com/ggml-org/llama.cpp/archive/refs/tags/$tag.tar.gz"
echo "$sha256  $work/src.tar.gz" | sha256sum --check --quiet
tar -xzf "$work/src.tar.gz" -C "$work"
src="$work/llama.cpp-$tag"

# The Pi 5's Cortex-A76 by name: the same instructions as -mcpu=native there
# (armv8.2-a, dotprod, fp16; ggml has no use for its crypto extension), so the
# binary does not depend on the machine that builds it. No web UI: the build
# would download its latest assets from Hugging Face, pinned by nothing.
cmake -S "$src" -B "$work/build" -DCMAKE_BUILD_TYPE=Release \
    -DGGML_NATIVE=OFF -DGGML_CPU_ARM_ARCH=armv8.2-a+dotprod+fp16 \
    -DCMAKE_C_FLAGS=-mtune=cortex-a76 -DCMAKE_CXX_FLAGS=-mtune=cortex-a76 \
    -DBUILD_SHARED_LIBS=OFF -DLLAMA_OPENSSL=OFF -DLLAMA_BUILD_TESTS=OFF -DLLAMA_BUILD_EXAMPLES=OFF \
    -DLLAMA_USE_PREBUILT_UI=OFF
cmake --build "$work/build" --target llama-server -j"$(nproc)"

mkdir -p "$out"
install -m 755 "$work/build/bin/llama-server" "$out/$name"
strip "$out/$name"
cp "$src/LICENSE" "$out/LICENSE"
(cd "$out" && sha256sum "$name" > "$name.sha256")
cat "$out/$name.sha256"
