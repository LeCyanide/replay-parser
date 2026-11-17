# wows-render for the bot host: a small Debian image holding only the binary.
#
#   docker build -t wows-render .
#   docker run --rm -v /srv/wows-data:/data:ro -v "$PWD":/work wows-render \
#       /work/replay.wowsreplay --out /work/replay.mp4 --data /data
#
# A Node bot image can take just the binary instead:
#   COPY --from=wows-render /usr/local/bin/wows-render /usr/local/bin/
# (its base must provide libstdc++6, as Debian/Ubuntu images do).

# Base images; override to use a mirror when Docker Hub rate-limits, e.g.
#   --build-arg RUST_IMAGE=mirror.gcr.io/library/rust:1-bookworm
#   --build-arg RUNTIME_IMAGE=mirror.gcr.io/library/debian:bookworm-slim
ARG RUST_IMAGE=rust:1-bookworm
ARG RUNTIME_IMAGE=debian:bookworm-slim

FROM ${RUST_IMAGE} AS build
# nasm: OpenH264's x86 SIMD kernels (without it encoding is ~2x slower).
RUN apt-get update && apt-get install -y --no-install-recommends nasm && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY . .
RUN cargo build --release --locked -p wows-replay-render --bin wows-render \
    && strip target/release/wows-render

FROM ${RUNTIME_IMAGE}
# OpenH264 is C++: the binary links libstdc++ dynamically; Debian slim ships it.
COPY --from=build /src/target/release/wows-render /usr/local/bin/wows-render
COPY render/assets/Inter-OFL.txt /usr/share/doc/wows-render/Inter-OFL.txt
COPY LICENSE /usr/share/doc/wows-render/LICENSE
USER nobody
ENTRYPOINT ["wows-render"]
