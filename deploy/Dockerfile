FROM debian:bookworm-slim@sha256:abd67ffcfa541b485a3dff59865ab629aa048a6c613e639d36e7456b0b229241 AS runtime

ARG OCI_SOURCE=""
ARG OCI_REVISION=""
LABEL org.opencontainers.image.source="$OCI_SOURCE" \
      org.opencontainers.image.revision="$OCI_REVISION" \
      org.opencontainers.image.licenses="MIT"

RUN apt-get update \
    && apt-get install --yes --no-install-recommends ca-certificates curl openssh-client tzdata \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --gid 10001 wareboxes \
    && useradd --uid 10001 --gid 10001 --no-create-home --home-dir /nonexistent \
        --shell /usr/sbin/nologin wareboxes

WORKDIR /opt/wareboxes
ENV RUST_LOG=info,wareboxes_server=info,sqlx::query=error \
    LOG_FORMAT=json
STOPSIGNAL SIGTERM

FROM runtime AS api
COPY --chown=10001:10001 wareboxes-server /usr/local/bin/wareboxes-server
COPY --chown=10001:10001 site /opt/wareboxes/site
ENV BIND_ADDR=0.0.0.0:8080 \
    ALLOW_PUBLIC_REGISTRATION=false \
    CORS_ALLOWED_ORIGINS= \
    MAX_REQUEST_BODY_BYTES=1048576 \
    MAX_IN_FLIGHT_REQUESTS=256 \
    REQUEST_RATE_LIMIT_PER_SECOND=1000 \
    LOGIN_RATE_LIMIT_PER_MINUTE=60 \
    REQUEST_TIMEOUT_SECONDS=30 \
    WEB_SESSION_ABSOLUTE_TTL_SECONDS=43200 \
    WEB_SESSION_IDLE_TTL_SECONDS=1800 \
    SECURE_WEB_SESSION_COOKIE=true \
    LEPTOS_OUTPUT_NAME=wareboxes-web \
    LEPTOS_SITE_ROOT=/opt/wareboxes/site \
    LEPTOS_SITE_PKG_DIR=pkg \
    LEPTOS_ENV=PROD
USER 10001:10001
EXPOSE 8080
HEALTHCHECK --interval=10s --timeout=3s --start-period=30s --retries=3 \
    CMD ["curl", "--fail", "--silent", "--show-error", "http://127.0.0.1:8080/health/ready"]
ENTRYPOINT ["/usr/local/bin/wareboxes-server"]

FROM runtime AS worker
COPY --chown=10001:10001 wareboxes-worker /usr/local/bin/wareboxes-worker
USER 10001:10001
ENTRYPOINT ["/usr/local/bin/wareboxes-worker"]
