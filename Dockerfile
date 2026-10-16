# syntax=docker/dockerfile:1
FROM --platform=$BUILDPLATFORM rust:1.99.1-slim-trixie AS builder
ARG TARGETARCH
ARG BUILDARCH
ARG ZIG_VERSION=0.16.0
ARG CARGO_ZIGBUILD_VERSION=0.23.4
ARG APP_VERSION=0.0.0-dev

RUN apt-get update \
 && apt-get install -y --no-install-recommends curl xz-utils ca-certificates \
 && rm -rf /var/lib/apt/lists/*

RUN set -eux; \
    case "$BUILDARCH" in amd64) HA=x86_64 ;; arm64) HA=aarch64 ;; *) echo "unsupported build arch $BUILDARCH"; exit 1 ;; esac; \
    curl -fsSL "https://ziglang.org/download/${ZIG_VERSION}/zig-${HA}-linux-${ZIG_VERSION}.tar.xz" | tar -xJ -C /opt; \
    ln -s "/opt/zig-${HA}-linux-${ZIG_VERSION}/zig" /usr/local/bin/zig; \
    curl -fsSL "https://github.com/rust-cross/cargo-zigbuild/releases/download/v${CARGO_ZIGBUILD_VERSION}/cargo-zigbuild-${HA}-unknown-linux-gnu.tar.xz" \
      | tar -xJ -C /usr/local/bin --strip-components=1; \
    zig version; \
    cargo-zigbuild --version

RUN set -eux; \
    case "$TARGETARCH" in amd64) T=x86_64-unknown-linux-musl ;; arm64) T=aarch64-unknown-linux-musl ;; *) echo "unsupported target arch $TARGETARCH"; exit 1 ;; esac; \
    echo "$T" > /rust-target; \
    rustup target add "$T"

WORKDIR /src
COPY . .
ENV APP_VERSION=${APP_VERSION}
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target,id=target-${TARGETARCH} \
    set -eux; \
    T="$(cat /rust-target)"; \
    cargo zigbuild --release --locked -p mcp-icloud-mail --target "$T"; \
    cp "target/$T/release/mcp-icloud-mail" /mcp-icloud-mail

FROM scratch
COPY --from=builder /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/ca-certificates.crt
COPY --from=builder /mcp-icloud-mail /mcp-icloud-mail
USER 65532:65532
EXPOSE 8443
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 CMD ["/mcp-icloud-mail", "--healthcheck"]
ENTRYPOINT ["/mcp-icloud-mail"]
