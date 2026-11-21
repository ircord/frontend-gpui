# Build environment for ircord-gpui: the Linux packages GPUI needs, clang and
# lld for the Windows cross-build, and cargo-xwin. `cargo xtask` builds this
# image on first use and runs every compile step inside it.
FROM rust:1.99-bookworm

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        clang lld llvm pkg-config cmake \
        libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev libx11-xcb-dev \
        libxcb1-dev libfontconfig1-dev libfreetype6-dev libvulkan-dev \
        libdbus-1-dev libssl-dev libzstd-dev \
    && rm -rf /var/lib/apt/lists/*

RUN rustup component add rustfmt clippy \
    && rustup target add x86_64-pc-windows-msvc \
    && cargo install cargo-xwin --locked --version 0.23.1 \
    && rm -rf /usr/local/cargo/registry /usr/local/cargo/git \
    && chmod -R a+rwX /usr/local/cargo /usr/local/rustup
