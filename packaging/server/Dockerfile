# carnet-server in a container: one binary, one volume, and the web
# viewer it serves at /.
#
#   docker build -f packaging/server/Dockerfile -t carnet-server .
#   docker run -d --name carnet -v carnet:/data -p 8080:8080 \
#       -e CARNET_ORIGIN=https://notes.example.com carnet-server
#   docker logs carnet          # the first invite code
#   docker exec carnet carnet-server invite
#
# Built from the repository's root: the server shares crates/carnet-sync
# with the app, and the viewer is crates/carnet-web as wasm under web/.
# Only the server, the viewer and what they need are compiled.
FROM rust:1-bookworm AS build
WORKDIR /src
RUN rustup target add wasm32-unknown-unknown \
 && cargo install --locked wasm-bindgen-cli --version 0.2.129
COPY . .
RUN cargo build --release --locked -p carnet-server \
 && cp target/release/carnet-server /carnet-server \
 && sh web/scripts/wasm.sh

# The viewer's page: Vite, then brotli and gzip copies for the server to
# send as they are.
FROM node:24-bookworm-slim AS web
RUN apt-get update && apt-get install -y --no-install-recommends brotli \
 && rm -rf /var/lib/apt/lists/*
WORKDIR /web
COPY web/package.json web/package-lock.json ./
RUN npm ci
COPY --from=build /src/web ./
RUN npx vite build && sh scripts/compress.sh

FROM debian:bookworm-slim
RUN useradd --system --uid 10001 --home-dir /data carnet \
 && mkdir /data && chown carnet /data
COPY --from=build /carnet-server /usr/local/bin/carnet-server
COPY --from=web /web/dist /usr/share/carnet/web
USER carnet
ENV CARNET_DATA=/data CARNET_LISTEN=0.0.0.0:8080 CARNET_WEB=/usr/share/carnet/web
VOLUME /data
EXPOSE 8080
# The server answers /v1/health while its hub does; `health` asks it.
HEALTHCHECK --interval=30s --timeout=10s --start-period=10s CMD ["carnet-server", "health"]
ENTRYPOINT ["carnet-server"]
