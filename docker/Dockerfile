# A Linux build environment for the workspace, matching what the engine
# targets. Useful on machines without a Rust toolchain, and on Windows and
# macOS where the windowing and Vulkan stack differs from the target platform.
#
#   docker build -t vx-build docker/
#   docker run --rm -v "$PWD:/work" -v vx-target:/target vx-build cargo test --workspace
#
# The named volume keeps build output off the bind mount; see CARGO_TARGET_DIR
# below.

FROM rust:1.94-bookworm

# winit needs the X11/Wayland client libraries to link; wgpu needs a Vulkan
# loader. mesa-vulkan-drivers supplies lavapipe, the software rasteriser the
# render tests run against so no GPU is required.
RUN apt-get update && apt-get install -y --no-install-recommends \
        libx11-dev \
        libxkbcommon-dev \
        libxkbcommon-x11-dev \
        libwayland-dev \
        libxcursor-dev \
        libxrandr-dev \
        libxi-dev \
        libvulkan1 \
        vulkan-tools \
        mesa-vulkan-drivers \
        pkg-config \
        cmake \
    && rm -rf /var/lib/apt/lists/*

RUN rustup component add clippy rustfmt

# Keep build output off the bind-mounted host filesystem: it is markedly slower
# across the boundary, and cargo's file locking is happier on a native volume.
ENV CARGO_TARGET_DIR=/target

# Debian installs the lavapipe ICD arch-suffixed, so the unqualified
# lvp_icd.json path does not exist here. Pointing at a file that is not there
# is not a loud failure — the loader reports no driver and the render tests
# skip themselves, passing without rendering anything.
ENV VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/lvp_icd.x86_64.json

WORKDIR /work
