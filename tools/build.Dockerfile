# Build environment for ircord-gpui: the Linux packages GPUI needs, clang,
# lld and llvm-rc for the Windows cross-build, cargo-xwin, and cargo-packager
# with NSIS for the installers, and sccache so checkouts share compiled
# dependencies. `cargo xtask` builds this image on first use
# and runs every compile step inside it.
FROM rust:1.99-bookworm

# Build libraries: GPUI's (X11, Wayland, fonts, Vulkan), D-Bus for the tray
# and notifications, and ALSA for the notification sound's audio output.
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        clang lld llvm pkg-config cmake \
        libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev libx11-xcb-dev \
        libxcb1-dev libfontconfig1-dev libfreetype6-dev libvulkan-dev \
        libdbus-1-dev libssl-dev libzstd-dev libasound2-dev \
        libgstreamer1.0-dev libgstreamer-plugins-base1.0-dev \
    && rm -rf /var/lib/apt/lists/*

# A desktop without a screen, for `cargo xtask capture`: a virtual X display,
# software Vulkan, a session bus with GNOME Keyring for the sign-in, and tools
# to type into the window and capture it.
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        xvfb xauth xdotool x11-utils imagemagick \
        mesa-vulkan-drivers libvulkan1 \
        dbus dbus-bin gnome-keyring libsecret-tools \
        fonts-dejavu-core \
        gstreamer1.0-plugins-base gstreamer1.0-plugins-good gstreamer1.0-libav gstreamer1.0-tools \
    && rm -rf /var/lib/apt/lists/*

# The installers: cargo-packager builds the .deb and, with NSIS's makensis,
# the Windows setup; minisign checks update signatures.
RUN apt-get update \
    && apt-get install -y --no-install-recommends nsis minisign \
    && rm -rf /var/lib/apt/lists/*

RUN rustup component add rustfmt clippy \
    && rustup target add x86_64-pc-windows-msvc aarch64-apple-darwin x86_64-apple-darwin \
    && cargo install cargo-xwin --locked --version 0.23.1 \
    && cargo install cargo-packager --locked --version 0.11.8 \
    && cargo install sccache --locked --version 0.19.0 --no-default-features \
    && rm -rf /usr/local/cargo/registry /usr/local/cargo/git \
    && chmod -R a+rwX /usr/local/cargo /usr/local/rustup
