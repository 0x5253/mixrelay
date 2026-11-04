# syntax=docker/dockerfile:1
ARG DISTRO_IMAGE
FROM rust:1.96.0-bookworm@sha256:5e2214abe154fe26e39f64488952e5c991eeed1d6d6da7cc8381ae83927f0cfc AS rust
FROM node:24.19.0-bookworm-slim@sha256:a9f5f7c91a432850b2a8a7797adf5eadb6c733ceed61167806cee7ea7fbc29df AS node
FROM ${DISTRO_IMAGE}

# Minimal distro images may lack roots before apt downloads ca-certificates.
# Kali's mirror redirects can already require HTTPS during that bootstrap.
COPY --from=rust /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/ca-certificates.crt
COPY infra/lab/install-linux-deps.sh /opt/mixrelay-lab/install-linux-deps.sh
COPY infra/lab/kali.sources /opt/mixrelay-lab/kali.sources
RUN MIXRELAY_LAB_CONTAINER=1 bash /opt/mixrelay-lab/install-linux-deps.sh
COPY --from=rust /usr/local/cargo /usr/local/cargo
COPY --from=rust /usr/local/rustup /usr/local/rustup
COPY --from=node /usr/local/bin/node /usr/local/bin/node
COPY --from=node /usr/local/lib/node_modules/npm /usr/local/lib/node_modules/npm
ENV PATH="/usr/local/cargo/bin:${PATH}" \
    RUSTUP_HOME=/usr/local/rustup CARGO_HOME=/usr/local/cargo
RUN node /usr/local/lib/node_modules/npm/bin/npm-cli.js install --global pnpm@11.9.0 \
    && useradd --create-home --uid 10001 lab \
    && chmod -R a+rX /usr/local/rustup /usr/local/cargo \
    && mkdir /workspace && chown lab:lab /workspace

# Keep guest dependencies and build products away from the macOS checkout. No
# Docker socket, host audio socket, home directory, or elevated runtime is mounted.
WORKDIR /workspace
COPY --chown=lab:lab . .
USER lab
ENV CARGO_HOME=/home/lab/.cargo CARGO_BUILD_JOBS=2 CI=true
RUN pnpm install --frozen-lockfile
ENTRYPOINT ["node", "scripts/lab/native-check.mjs"]
CMD ["build"]
