# chrome-cdp's E2E image (docs/e2e/determinism.md): the same Chrome and the
# same fonts on every machine, so `expect-screenshot` baselines made in it
# match everywhere it runs. Node is in it for the web servers of
# [e2e.web_server].
#
#   docker build -f docs/e2e/Dockerfile -t chrome-cdp-e2e .
#   docker run --rm -v "$PWD":/app -w /app chrome-cdp-e2e test
#
# Run the build from the chrome-cdp checkout; run the tests from your app.

FROM rust:1-bookworm AS build
WORKDIR /src
COPY . .
RUN cargo install --locked --path crates/chrome-cdp --root /out

FROM node:22-bookworm-slim
# The fonts pages fall back to: Latin (Liberation is metric-compatible
# with Arial, Times and Courier), DejaVu, Noto for other scripts, and
# color emoji. Nothing else is installed, so every run sees the same set.
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
       ca-certificates fontconfig fonts-liberation fonts-dejavu-core \
       fonts-noto-core fonts-noto-cjk fonts-noto-color-emoji \
    && rm -rf /var/lib/apt/lists/*
COPY --from=build /out/bin/chrome-cdp /usr/local/bin/chrome-cdp
# One pinned Chrome for Testing, and the libraries it needs.
ENV CHROME_CDP_CACHE=/opt/chrome-cdp
ARG CHROME_VERSION=154
RUN apt-get update \
    && chrome-cdp install --version "$CHROME_VERSION" --with-deps \
    && rm -rf /var/lib/apt/lists/*
ENTRYPOINT ["chrome-cdp"]
CMD ["test"]
