# The server and web images (Technical Plan P5.2; decision log D-065 and D-066). The server: one
# build stage compiles it after a cargo-chef layer of its dependencies, and the final target copies
# only the binary onto a distroless base: glibc and CA certificates, no shell, no package manager,
# no compiler. The web: a Node stage builds the static app, and the final target holds only the
# built files, static-web-server and a health client, both single static binaries with no shell.
# Both run as 65532:65532. Build one with `just images`, or:
#   docker build -f dockerfiles/Dockerfile --target server -t voidforge-server:local .
#   docker build -f dockerfiles/Dockerfile --target web -t voidforge-web:local .
# Every base is pinned by digest (an OCI index, so amd64 and arm64 both resolve); the pin check in
# .github/scripts/check-images.py refuses a FROM without one.

FROM rust:1.99.1-slim-trixie@sha256:aab42391a986424e22d6a3ec14f37bc6dd3dacff4d457ca7cccd781e02297cc6 AS chef
# The base is the toolchain rust-toolchain.toml names; RUSTUP_TOOLCHAIN keeps rustup from fetching
# that file's clippy and rustfmt, which a release build doesn't use. aws-lc-sys needs only the C
# compiler the base already has (D-046, amendment 1).
ENV RUSTUP_TOOLCHAIN=1.99.1 \
    SQLX_OFFLINE=true \
    CARGO_TERM_COLOR=never
# cargo-chef (Technical Plan, Tech stack: images) splits the dependencies into a layer of their own,
# cached by both the legacy builder and BuildKit, so a source change doesn't rebuild them. It's a
# build-stage tool, pinned here and in docs/versions.md, and never reaches a runtime image.
RUN cargo install cargo-chef --locked --version 0.1.78
WORKDIR /src

# The dependency recipe: it changes only when a manifest or the lock file does.
FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
# Parallelism stays configurable: a 4 GiB Docker VM may need fewer jobs than CPUs (the advisor's
# boundary note: lower this, never resize the VM).
ARG CARGO_BUILD_JOBS=2
COPY --from=planner /src/recipe.json recipe.json
RUN CARGO_BUILD_JOBS="$CARGO_BUILD_JOBS" \
    cargo chef cook --release --locked --recipe-path recipe.json -p voidforge-server
COPY . .
# The engine build's identity in snapshot headers (D-035): .git isn't in the build context. Declared
# after the dependency layer, so a new commit doesn't invalidate it.
ARG VOIDFORGE_GIT_SHA=unknown
RUN CARGO_BUILD_JOBS="$CARGO_BUILD_JOBS" VOIDFORGE_GIT_SHA="$VOIDFORGE_GIT_SHA" \
    cargo build --release --locked -p voidforge-server

FROM gcr.io/distroless/cc-debian13:nonroot@sha256:83b9b50586253821cf63ffcec87d3d94265a093ad30da232039af962dd8f4edf AS server
COPY --from=builder /src/target/release/voidforge-server /usr/local/bin/voidforge-server
COPY rules/ /etc/voidforge/rules/
USER 65532:65532
# Exec form: serve is PID 1 and receives `docker stop`'s SIGTERM itself (D-050 I5).
ENTRYPOINT ["/usr/local/bin/voidforge-server"]

# The static app, built once per environment: Vite bakes the API's URL and the issuer into the
# bundle, so they're build arguments, with the local stack's defaults.
FROM node:26.12.0-trixie-slim@sha256:e556a144463cc8dc53afe8de6d2fa0db5c132fd39ffa232c17641bd40df2bf11 AS app
WORKDIR /web
COPY web/package.json web/package-lock.json ./
RUN npm ci --no-audit --no-fund
COPY web/ ./
ARG VITE_API_URL=http://localhost:8080
ARG VITE_OIDC_ISSUER=http://localhost:5556/dex
RUN npm run build
# The served CSP is the built page's own meta policy plus frame-ancestors, which only a header can
# carry, so the two never disagree.
COPY dockerfiles/static-web-server.toml /web/static-web-server.toml
RUN node -e ' \
    const fs = require("fs"); \
    const page = fs.readFileSync("dist/index.html", "utf8"); \
    const meta = /http-equiv="Content-Security-Policy" content="([^"]+)"/.exec(page); \
    if (!meta) throw new Error("the built index.html has no CSP meta tag"); \
    const csp = meta[1].replaceAll("&#39;", "\x27") + "; frame-ancestors \x27none\x27"; \
    const toml = fs.readFileSync("static-web-server.toml", "utf8"); \
    if (!toml.includes("CSP_PLACEHOLDER")) throw new Error("no CSP_PLACEHOLDER in the config"); \
    fs.writeFileSync("static-web-server.toml", toml.replace("CSP_PLACEHOLDER", csp));'

# The health check's client: one static binary, no shell (D-066).
FROM ghcr.io/tarampampam/microcheck:1.4.0@sha256:c9f79cd408626de7c10f2d487d67339f49adf0ba61dde96ede65343269db1f85 AS healthcheck

FROM joseluisq/static-web-server:2.44.1@sha256:94618e3d17802265a21926217e6f9e7832b311b6fbf5933684001739fafed06a AS web
ARG VITE_API_URL=http://localhost:8080
ARG VITE_OIDC_ISSUER=http://localhost:5556/dex
# What the bundle calls, so a run can refuse an image built for another API.
LABEL org.voidforge.api-url="$VITE_API_URL" org.voidforge.issuer="$VITE_OIDC_ISSUER"
COPY --from=app /web/dist/ /public/
COPY --from=healthcheck /bin/httpcheck /usr/local/bin/httpcheck
COPY --from=app /web/static-web-server.toml /etc/static-web-server.toml
# dex registers the redirect by exact string, http://localhost:8081/callback, so the app serves
# there. The config file's bind loses to the server's CLI defaults, so the bind is in the env.
ENV SERVER_HOST=127.0.0.1 \
    SERVER_PORT=8081 \
    SERVER_CONFIG_FILE=/etc/static-web-server.toml
USER 65532:65532
# The base stops on SIGQUIT; `docker stop` sends this, and the server exits at once (grace 0).
STOPSIGNAL SIGTERM
ENTRYPOINT ["/static-web-server"]
