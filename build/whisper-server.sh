#!/bin/bash
# Builds the whisper-server the package installs, for a Raspberry Pi 5 on
# Debian 12, as build/llama-server.sh does for llama-server. Run by
# .github/workflows/whisper-server.yml in a debian:12 container; runs the same
# way on the Pi. Writes, in the output folder:
# whisper-server-<tag>-debian12-arm64, its .sha256 and whisper.cpp's LICENSE.
#
# usage: build/whisper-server.sh <output folder>
set -euo pipefail

tag=v1.9.6
sha256=7ab15dc88022d2425ad94dcffd8abbf1b6d7018089b4e48d6b40780738e0b445
out=$(realpath "$1")
name="whisper-server-$tag-debian12-arm64"

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
curl --fail --silent --show-error --location --output "$work/src.tar.gz" \
    "https://github.com/ggml-org/whisper.cpp/archive/refs/tags/$tag.tar.gz"
echo "$sha256  $work/src.tar.gz" | sha256sum --check --quiet
tar -xzf "$work/src.tar.gz" -C "$work"
src="$work/whisper.cpp-${tag#v}"

# The Pi 5's Cortex-A76 by name, as for llama-server. The server is one of
# the examples; only its target is built. No SDL, no curl, no ffmpeg: the
# tool server decodes the audio itself and sends 16 kHz WAV.
cmake -S "$src" -B "$work/build" -DCMAKE_BUILD_TYPE=Release \
    -DGGML_NATIVE=OFF -DGGML_CPU_ARM_ARCH=armv8.2-a+dotprod+fp16 \
    -DCMAKE_C_FLAGS="-mtune=cortex-a76 -ffile-prefix-map=$work=." \
    -DCMAKE_CXX_FLAGS="-mtune=cortex-a76 -ffile-prefix-map=$work=." \
    -DBUILD_SHARED_LIBS=OFF -DWHISPER_BUILD_TESTS=OFF -DWHISPER_BUILD_EXAMPLES=ON \
    -DWHISPER_SDL2=OFF -DWHISPER_CURL=OFF
cmake --build "$work/build" --target whisper-server -j"$(nproc)"

mkdir -p "$out"
install -m 755 "$work/build/bin/whisper-server" "$out/$name"
strip "$out/$name"
cp "$src/LICENSE" "$out/LICENSE"
(cd "$out" && sha256sum "$name" > "$name.sha256")
cat "$out/$name.sha256"
