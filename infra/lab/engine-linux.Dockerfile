# Native Linux engine toolchain; no desktop/frontend build and no host audio access.
# Mount the checkout read-only at /workspace and separate named volumes at
# /workspace/target and /home/lab/.cargo. Never mount host sockets or home directories.
# Override with a verified digest for a separate distribution qualification image.
ARG RUST_IMAGE=rust:1.96.0-bookworm@sha256:5e2214abe154fe26e39f64488952e5c991eeed1d6d6da7cc8381ae83927f0cfc
FROM ${RUST_IMAGE}

RUN rustup component add clippy rustfmt
RUN apt-get -o APT::Update::Error-Mode=any update \
    && apt-get install -y --no-install-recommends \
       clang cmake pkg-config libasound2-dev liblilv-dev libpipewire-0.3-dev \
       libgdk-pixbuf-2.0-dev librsvg2-common \
       libssl-dev libx11-dev libxrandr-dev libxi-dev \
       pipewire pipewire-bin wireplumber dbus-x11 \
    && useradd --create-home --uid 10001 lab \
    && mkdir -p /workspace/target /home/lab/.cargo \
    && chown lab:lab /workspace/target /home/lab/.cargo

WORKDIR /workspace
USER lab
ENV CARGO_HOME=/home/lab/.cargo CARGO_TARGET_DIR=/workspace/target CARGO_BUILD_JOBS=2
CMD ["cargo", "test", "--locked", "-p", "mixrelay-engine-service", "--lib", "--", "--test-threads=1"]
