# SAP-Automate server — multi-stage build.
#
# Stage 1 builds a static-ish release binary using musl when available;
# falls back to glibc-linked otherwise.  The runtime stage is a distroless
# base so we ship no shell / package manager.

ARG RUST_VERSION=1.84

FROM rust:${RUST_VERSION}-slim-bookworm AS builder

WORKDIR /src

# Install the few system deps the workspace needs:
#  - pkg-config + libssl for reqwest's rustls fallback path
#  - ca-certificates for outbound HTTPS to SAP / OpenAI
RUN apt-get update \
 && apt-get install -y --no-install-recommends \
        pkg-config libssl-dev ca-certificates \
 && rm -rf /var/lib/apt/lists/*

# Pre-cache dependencies so iterative builds are fast.
COPY Cargo.toml Cargo.lock ./
COPY crates/ ./crates/
COPY apps/ ./apps/

# Build the two binaries that production needs.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release \
        --bin sap-automate-server \
        --bin sap-automate-gw \
        --features sap-automate-adt/http \
 && cp /src/target/release/sap-automate-server /usr/local/bin/ \
 && cp /src/target/release/sap-automate-gw     /usr/local/bin/

# Runtime stage: distroless cc base (≈ 20 MB).
FROM gcr.io/distroless/cc-debian12:nonroot AS runtime

COPY --from=builder /usr/local/bin/sap-automate-server /usr/local/bin/
COPY --from=builder /usr/local/bin/sap-automate-gw     /usr/local/bin/
COPY --from=builder /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/

# Default to the server binary; the gateway is selectable per-pod via
# Kubernetes `command:` override.
USER nonroot:nonroot
EXPOSE 3030
ENTRYPOINT ["/usr/local/bin/sap-automate-server"]
CMD ["--transport", "http", "--bind", "0.0.0.0:3030"]
