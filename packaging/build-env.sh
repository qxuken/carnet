#!/bin/sh
# Turns a Debian bookworm into Carnet's build environment for Linux and
# Windows: packaging/Dockerfile runs it to make the local build image, and
# .github/workflows/release.yml runs it in the CI job's container, so the
# two cannot drift. Run as root; Rust goes to /usr/local/{cargo,rustup}
# unless CARGO_HOME and RUSTUP_HOME say otherwise, and its bin to the PATH
# is the caller's to add.
#
#   Linux    cargo-zigbuild and zig, pinned as in kui's release, so the
#            binaries need no newer glibc than 2.28; ALSA's headers for both
#            architectures (kui's sound links libasound)
#   Windows  cargo-xwin, with LLVM's clang-cl, lld-link, llvm-lib and
#            llvm-rc (build.rs's icon) under their plain names; NSIS
#   both     Nushell, which scripts/package.nu is written in
set -eux

NU_VERSION=0.116.0
CARGO_ZIGBUILD_VERSION=0.23.3
ZIGLANG_VERSION=0.16.0
CARGO_XWIN_VERSION=0.23.1   # and in release.yml's cache key

export CARGO_HOME="${CARGO_HOME:-/usr/local/cargo}"
export RUSTUP_HOME="${RUSTUP_HOME:-/usr/local/rustup}"
arch=$(uname -m)

dpkg --add-architecture amd64
dpkg --add-architecture arm64
apt-get update
apt-get install -y --no-install-recommends ca-certificates curl pkg-config python3-pip zip \
    nsis clang lld llvm libasound2-dev:amd64 libasound2-dev:arm64
rm -rf /var/lib/apt/lists/*

# Debian names LLVM's tools by version (clang-cl-14); cargo-xwin and
# embed-resource look for the plain names.
llvm=$(ls -d /usr/lib/llvm-*/bin | sort -V | tail -n 1)
for tool in clang-cl lld-link llvm-lib llvm-rc; do
    ln -sf "$llvm/$tool" "/usr/local/bin/$tool"
done

curl -fsSL https://sh.rustup.rs | sh -s -- -y --no-modify-path --profile minimal \
    --default-toolchain stable \
    -t x86_64-unknown-linux-gnu -t aarch64-unknown-linux-gnu -t x86_64-pc-windows-msvc

pip3 install --break-system-packages \
    "cargo-zigbuild==${CARGO_ZIGBUILD_VERSION}" "ziglang==${ZIGLANG_VERSION}"

curl -fL --retry 3 \
    "https://github.com/rust-cross/cargo-xwin/releases/download/v${CARGO_XWIN_VERSION}/cargo-xwin-v${CARGO_XWIN_VERSION}.${arch}-unknown-linux-musl.tar.gz" \
    | tar -xz -C /usr/local/bin cargo-xwin

dir="nu-${NU_VERSION}-${arch}-unknown-linux-gnu"
curl -fL --retry 3 "https://github.com/nushell/nushell/releases/download/${NU_VERSION}/${dir}.tar.gz" \
    | tar -xz -C /usr/local/bin --strip-components=1 "${dir}/nu"

"$CARGO_HOME/bin/cargo" --version
cargo-xwin --version
cargo-zigbuild --version
makensis -VERSION
nu --version
