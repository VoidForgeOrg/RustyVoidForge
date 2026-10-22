# The server and web images (Technical Plan P5.2; decision log D-065). One build stage compiles
# both binaries once, after a cargo-chef layer of their dependencies; each final target copies only its own binary onto a distroless base: glibc
# and CA certificates, no shell, no package manager, no compiler, running as `nonroot`
# (65532:65532). Build one with `just images`, or:
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
    cargo chef cook --release --locked --recipe-path recipe.json -p voidforge-server -p voidforge-web
COPY . .
# The engine build's identity in snapshot headers (D-035): .git isn't in the build context. Declared
# after the dependency layer, so a new commit doesn't invalidate it.
ARG VOIDFORGE_GIT_SHA=unknown
RUN CARGO_BUILD_JOBS="$CARGO_BUILD_JOBS" VOIDFORGE_GIT_SHA="$VOIDFORGE_GIT_SHA" \
    cargo build --release --locked -p voidforge-server -p voidforge-web

FROM gcr.io/distroless/cc-debian13:nonroot@sha256:83b9b50586253821cf63ffcec87d3d94265a093ad30da232039af962dd8f4edf AS server
COPY --from=builder /src/target/release/voidforge-server /usr/local/bin/voidforge-server
COPY rules/ /etc/voidforge/rules/
USER 65532:65532
# Exec form: serve is PID 1 and receives `docker stop`'s SIGTERM itself (D-050 I5).
ENTRYPOINT ["/usr/local/bin/voidforge-server"]

FROM gcr.io/distroless/cc-debian13:nonroot@sha256:83b9b50586253821cf63ffcec87d3d94265a093ad30da232039af962dd8f4edf AS web
COPY --from=builder /src/target/release/voidforge-web /usr/local/bin/voidforge-web
USER 65532:65532
ENTRYPOINT ["/usr/local/bin/voidforge-web"]
