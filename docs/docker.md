# Docker

Every release publishes an image with chrome-cdp, one pinned Chrome for
Testing with the system libraries it needs, and a fixed set of fonts:

```sh
docker pull ghcr.io/glotlabs/chrome-cdp          # latest release; or :1.2.3
docker run --rm ghcr.io/glotlabs/chrome-cdp doctor
```

It is built for `linux/amd64` and `linux/arm64` from
[docker/Dockerfile](../docker/Dockerfile): Debian bookworm-slim, the
release's static binary in `/usr/local/bin`, Chrome in `/opt/chrome-cdp`
(`CHROME_CDP_CACHE`), and a user `chrome` (uid 1000) with `/work` as its
working directory. The fonts are Liberation (metric-compatible with Arial,
Times and Courier), DejaVu, Noto (other scripts and CJK) and Noto Color
Emoji, and nothing else, so a page renders the same in every container,
which is what `expect-screenshot` baselines need.

## A session in a container

A session's daemon keeps running between commands, so start the container
once and run commands in it with `docker exec`. `--init` reaps the
processes Chrome leaves behind:

```sh
docker run -d --init --name browser --entrypoint sleep ghcr.io/glotlabs/chrome-cdp infinity
docker exec browser chrome-cdp open https://example.com
docker exec browser chrome-cdp snapshot -i
docker exec browser chrome-cdp screenshot -o /work/page.png
docker cp browser:/work/page.png .
docker rm -f browser
```

A single `docker run --rm … chrome-cdp <command>` works too, but the
container (and its browser) is gone when that command ends; use `batch`
or `run` to do several steps in one run:

```sh
docker run --rm -i ghcr.io/glotlabs/chrome-cdp batch <<'EOF'
open https://example.com
snapshot -i
text css=h1
EOF
```

To test a dev server on the host, use the host's network
(`--network host` on Linux) or `host.docker.internal` (Docker Desktop, or
`--add-host host.docker.internal:host-gateway` on Linux):

```sh
docker run --rm -i --network host ghcr.io/glotlabs/chrome-cdp batch < smoke.cdp
```

## E2E tests

Mount the project and run `chrome-cdp test` in it. An app whose web
server needs Node (`[e2e.web_server]`) needs Node in the image; extend it:

```dockerfile
FROM ghcr.io/glotlabs/chrome-cdp
USER root
RUN apt-get update && apt-get install -y --no-install-recommends nodejs npm \
    && rm -rf /var/lib/apt/lists/*
USER chrome
```

```sh
docker run --rm --init -v "$PWD":/work ghcr.io/glotlabs/chrome-cdp test
```

The container's user is uid 1000; with another uid on the host, add
`--user "$(id -u):$(id -g)"` so the results in `e2e/.results/` are
yours. [e2e/determinism.md](e2e/determinism.md) explains why screenshot
baselines should be made in the same image that checks them;
[docs/e2e/Dockerfile](e2e/Dockerfile) builds such an image, with Node,
from a checkout of chrome-cdp.

## The sandbox

Chrome's sandbox needs user namespaces. Docker's default seccomp profile
blocks them, so in a plain `docker run` chrome-cdp starts Chrome with
`--no-sandbox` and warns:

```
warning: running Chrome without its sandbox (--no-sandbox): this looks like a container (/.dockerenv exists) that cannot create user namespaces; a malicious page is far more dangerous
```

That is fine for your own app and pages you trust. For anything else keep
the sandbox on: allow the namespaces with `--security-opt
seccomp=unconfined` (or a seccomp profile that permits `clone` and
`unshare` with `CLONE_NEWUSER`). `chrome-cdp doctor`'s `launch` line says
which one you got. The image never runs Chrome as root; running it as
root (`--user root`) turns the sandbox off too. [security.md](security.md)
has the rest of the security model.

## Pinning Chrome

Each image holds the Chrome for Testing stable that was current when it
was built, so a given image tag always runs the same Chrome. To pin
another version, build the image yourself from the release's binaries
(the Dockerfile's header shows how) with `--build-arg CHROME_VERSION=153`
(a major version or a full one, as `chrome-cdp install --version` takes).
