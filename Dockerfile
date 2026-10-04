# syntax=docker/dockerfile:1.7
#
# Hoglet container image: one static binary on an empty base.
#
#   docker build -t hoglet .
#   docker run -p 8000:8000 -v hoglet-data:/data hoglet
#
# By default the binary is compiled here: a fully static musl build via
# cargo-zigbuild, cross-compiled from the build platform for TARGETARCH, so a
# multi-arch image builds without QEMU-emulated rustc.
#
# Release CI passes --build-arg BINARY_SOURCE=prebuilt and supplies the
# already smoke-tested binaries at dist/docker/<amd64|arm64>/hoglet instead.
#
# Shutdown: Hoglet flushes cleanly on SIGINT, hence STOPSIGNAL below.
# Health: the image has no shell or curl. Probe HTTP GET /ready (readiness)
# or /health (liveness) from the orchestrator instead of a HEALTHCHECK.

ARG RUST_VERSION=1.97
ARG ZIG_VERSION=0.17.0
ARG CARGO_ZIGBUILD_VERSION=0.23.4
ARG BINARY_SOURCE=source

FROM --platform=$BUILDPLATFORM rust:${RUST_VERSION}-bookworm AS source
ARG ZIG_VERSION
ARG CARGO_ZIGBUILD_VERSION
ARG TARGETARCH
RUN set -eux; \
    zarch="$(uname -m)"; \
    curl -fsSL "https://ziglang.org/download/${ZIG_VERSION}/zig-${zarch}-linux-${ZIG_VERSION}.tar.xz" \
        | tar -xJ -C /opt; \
    ln -s "/opt/zig-${zarch}-linux-${ZIG_VERSION}/zig" /usr/local/bin/zig; \
    cargo install --locked cargo-zigbuild --version "${CARGO_ZIGBUILD_VERSION}"; \
    rustup target add x86_64-unknown-linux-musl aarch64-unknown-linux-musl
WORKDIR /src
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target,id=hoglet-target,sharing=locked \
    set -eux; \
    case "$TARGETARCH" in \
        amd64) triple=x86_64-unknown-linux-musl ;; \
        arm64) triple=aarch64-unknown-linux-musl ;; \
        *) echo "unsupported TARGETARCH $TARGETARCH" >&2; exit 1 ;; \
    esac; \
    cargo zigbuild --release --locked --target "$triple"; \
    install -D -m 0755 "target/$triple/release/hoglet" /out/hoglet; \
    mkdir -p /out/data

FROM --platform=$BUILDPLATFORM busybox:1.37 AS prebuilt
ARG TARGETARCH
COPY dist/docker/${TARGETARCH}/hoglet /out/hoglet
RUN chmod 0755 /out/hoglet && mkdir -p /out/data

FROM ${BINARY_SOURCE} AS binary

FROM scratch
COPY --from=binary /out/hoglet /hoglet
# Pre-created and owned by the non-root user, so a fresh named volume inherits it.
COPY --from=binary --chown=65532:65532 /out/data /data
USER 65532:65532
ENV HOGLET_ADDR=0.0.0.0:8000 \
    HOGLET_DATA=/data
VOLUME ["/data"]
EXPOSE 8000
STOPSIGNAL SIGINT
ENTRYPOINT ["/hoglet"]
