FROM debian:trixie-slim
RUN apt-get update && apt-get install -y --no-install-recommends curl ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=kanidm/server:1.10.3 /sbin/kanidmd /sbin/kanidmd
COPY --from=kanidm/server:1.10.3 /lib/libgcc_s.so.1 /lib/libgcc_s.so.1
