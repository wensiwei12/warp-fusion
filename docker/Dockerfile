# ===========================================================================
# Warp Fusion - Runtime Docker Image (Multi-arch)
# ===========================================================================

FROM ubuntu:24.04

ARG TARGETARCH=amd64

LABEL org.opencontainers.image.source="https://github.com/wp-labs/warp-fusion"
LABEL org.opencontainers.image.description="Warp Fusion runtime, rule tooling, and scenario generation tools"
LABEL org.opencontainers.image.licenses="Apache-2.0"

# Install runtime dependencies
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    libssl3t64 \
    libsqlite3-0 \
    zlib1g \
    libsasl2-2 \
    libzstd1 \
    libgit2-1.7 \
    && rm -rf /var/lib/apt/lists/* \
    && useradd -r -s /bin/false wfusion

# Copy pre-built binaries (provided via build context, arch-specific)
COPY ${TARGETARCH}/wfusion /usr/local/bin/
COPY ${TARGETARCH}/wfgen /usr/local/bin/
COPY ${TARGETARCH}/wfl /usr/local/bin/
COPY ${TARGETARCH}/wfadm /usr/local/bin/

# Create data directory and seed it with the vendored default setting.
# default_setting/ is committed (generated from `wfadm init` templates) so the
# image build does not need to execute wfadm — which would break under
# multi-arch cross-builds (cannot run a foreign-arch binary at build time).
RUN chmod 0755 /usr/local/bin/wfusion /usr/local/bin/wfgen /usr/local/bin/wfl /usr/local/bin/wfadm \
    && mkdir -p /data \
    && chown wfusion:wfusion /data

COPY --chown=wfusion:wfusion default_setting/ /data/

USER wfusion
WORKDIR /data

ENTRYPOINT ["wfusion"]
CMD ["--help"]
