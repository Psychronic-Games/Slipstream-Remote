# docker build -f packaging/server/Dockerfile -t slipstream-server .
# docker run -d --name slipstream -p 21116:21116/udp -p 21117:21117 \
#   -v slipstream-data:/data -e SLIPSTREAM_PUBLIC_HOST=your.server.example slipstream-server
FROM rust:1-bookworm AS build
WORKDIR /src
COPY . .
RUN cargo build --release -p slipstream-server

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=build /src/target/release/slipstream-server /usr/local/bin/slipstream-server
ENV SLIPSTREAM_DATA=/data
VOLUME /data
EXPOSE 21116/udp 21117/tcp
ENTRYPOINT ["slipstream-server"]
