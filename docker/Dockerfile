# The published image, ghcr.io/glotlabs/chrome-cdp (docs/docker.md):
# chrome-cdp, one pinned Chrome for Testing with the libraries it needs,
# and a fixed set of fonts, so pages render the same on every machine.
#
# The release workflow builds it for linux/amd64 and linux/arm64 from the
# release's static musl binaries. By hand, from the checkout:
#
#   cargo xtask dist --target x86_64-unknown-linux-musl
#   mkdir -p target/docker && cp dist/chrome-cdp-*-unknown-linux-musl target/docker/
#   docker build -f docker/Dockerfile -t chrome-cdp target/docker
#   docker run --rm chrome-cdp doctor

FROM debian:bookworm-slim

# The fonts pages fall back to: Latin (Liberation is metric-compatible
# with Arial, Times and Courier), DejaVu, Noto for other scripts, and
# color emoji. Nothing else is installed, so every run sees the same set.
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
       ca-certificates fontconfig fonts-liberation fonts-dejavu-core \
       fonts-noto-core fonts-noto-cjk fonts-noto-color-emoji \
    && rm -rf /var/lib/apt/lists/*

# The binary of the platform being built (buildx sets TARGETARCH).
ARG TARGETARCH
COPY chrome-cdp-*-unknown-linux-musl /tmp/bin/
RUN case "$TARGETARCH" in \
       amd64|"") arch=x86_64 ;; \
       arm64) arch=aarch64 ;; \
       *) echo "no chrome-cdp binary for $TARGETARCH" >&2; exit 1 ;; \
    esac \
    && install -m 0755 "/tmp/bin/chrome-cdp-$arch-unknown-linux-musl" /usr/local/bin/chrome-cdp \
    && rm -rf /tmp/bin

# One pinned Chrome for Testing, and the libraries it needs, in a cache
# every user can read.
ENV CHROME_CDP_CACHE=/opt/chrome-cdp
ARG CHROME_VERSION=stable
RUN apt-get update \
    && if [ "$CHROME_VERSION" = stable ]; then \
         chrome-cdp install --with-deps; \
       else \
         chrome-cdp install --version "$CHROME_VERSION" --with-deps; \
       fi \
    && rm -rf /var/lib/apt/lists/*

# Not root, so Chrome keeps its sandbox where the container allows user
# namespaces (docs/docker.md). Sessions live in XDG_RUNTIME_DIR.
RUN useradd --create-home --uid 1000 chrome \
    && mkdir -p /run/chrome-cdp /work \
    && chown chrome:chrome /run/chrome-cdp /work \
    && chmod 0700 /run/chrome-cdp
USER chrome
ENV XDG_RUNTIME_DIR=/run/chrome-cdp
WORKDIR /work
ENTRYPOINT ["chrome-cdp"]
CMD ["--help"]
