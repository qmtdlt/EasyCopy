FROM rust:1.90-bookworm AS builder
WORKDIR /build
# Build with the same committed workspace resolution used by the desktop.
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY apps/desktop/src-tauri ./apps/desktop/src-tauri
RUN cargo build --locked --release -p easycopy-relay

FROM debian:bookworm-slim
RUN groupadd --gid 10001 easycopy && useradd --uid 10001 --gid easycopy --no-create-home easycopy && mkdir /data && chown easycopy:easycopy /data
COPY --from=builder /build/target/release/easycopy-relay /usr/local/bin/easycopy-relay
USER easycopy
ENV EASYCOPY_DATA=/data EASYCOPY_BIND=0.0.0.0:8443
VOLUME ["/data"]
EXPOSE 8443
HEALTHCHECK --interval=30s --timeout=8s --start-period=10s --retries=3 CMD ["easycopy-relay", "healthcheck"]
ENTRYPOINT ["easycopy-relay"]
CMD ["serve"]
