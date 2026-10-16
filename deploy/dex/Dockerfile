# dex with this repository's config baked in. Baking avoids bind mounts, which fail when the
# Docker VM cannot read the checkout (for example colima on macOS without access to ~/Documents).
# DEX_CONFIG picks config.yaml (local) or config.ci.yaml (CI: adds the password grant).
FROM ghcr.io/dexidp/dex:v2.46.0@sha256:933fcd3f523338c847b88ef84a7145fb2fcb7b9917f08418612afeaaa2001777
ARG DEX_CONFIG=config.yaml
COPY ${DEX_CONFIG} /etc/dex/config.yaml
CMD ["dex", "serve", "/etc/dex/config.yaml"]
