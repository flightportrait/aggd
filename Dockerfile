# aggd as a small image: build with the Rust toolchain, run from scratch-like base.
FROM rust:1-bookworm AS build
WORKDIR /src
COPY . .
RUN cargo build --release -p aggd

FROM debian:bookworm-slim
RUN useradd --system --home /var/lib/aggd --create-home aggd
COPY --from=build /src/target/release/aggd /usr/local/bin/aggd
USER aggd
VOLUME ["/var/lib/aggd", "/var/globe_history"]
# 30004 Beast in from receivers, 8080 HTTP (data, API, map)
EXPOSE 30004 8080
ENTRYPOINT ["/usr/local/bin/aggd"]
CMD ["--net-bi-port", "30004", "--http-port", "8080", "--write-state", "/var/lib/aggd", "--write-globe-history", "/var/globe_history"]
