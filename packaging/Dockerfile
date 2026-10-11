# scripts/package.nu's build image, for the Linux binaries and the Windows
# installer when they are made on a Mac (Homebrew's makensis aborts on
# macOS 27). What goes in it is packaging/build-env.sh, which the release
# workflow runs in its own container too; node:24-bookworm is that
# container's image, so a local build and CI's start from the same system.
FROM node:24-bookworm
ENV CARGO_HOME=/usr/local/cargo RUSTUP_HOME=/usr/local/rustup
ENV PATH=/usr/local/cargo/bin:$PATH
COPY build-env.sh /tmp/build-env.sh
RUN sh /tmp/build-env.sh && rm /tmp/build-env.sh
# Tells scripts/package.nu it is already inside: build here, not in Docker.
ENV CARNET_BUILD_ENV=1
