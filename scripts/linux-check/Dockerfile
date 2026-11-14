# Linux check image: native Debian (arm64 on Apple silicon, amd64 elsewhere) with what CI, install.sh and
# the PKGBUILD need to build and test the workspace headless. Nothing here runs the real app.
FROM rust:1-trixie
RUN apt-get update -q && DEBIAN_FRONTEND=noninteractive apt-get install -yq --no-install-recommends \
      pkg-config build-essential git rsync ca-certificates curl xz-utils \
      libdbus-1-dev libasound2-dev libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev libssl-dev \
      libudev-dev libx11-dev libxcb1-dev libvulkan1 mesa-vulkan-drivers libegl1 libgl1-mesa-dri libgles2 \
      ffmpeg fonts-dejavu-core fonts-noto-core \
 && rm -rf /var/lib/apt/lists/*
# Node for the bridge tests (plain node:test files, no Electron needed).
RUN curl -fsSL https://nodejs.org/dist/v22.12.0/node-v22.12.0-linux-$(dpkg --print-architecture | sed 's/amd64/x64/;s/arm64/arm64/').tar.xz \
    | tar -xJ -C /usr/local --strip-components=1
RUN rustup component add clippy
ENV HOME=/tmp/home CARGO_HOME=/cargo CARGO_TARGET_DIR=/target CARGO_INCREMENTAL=0 \
    CARGO_PROFILE_DEV_DEBUG=line-tables-only UZUMAKI_MUTE=1
