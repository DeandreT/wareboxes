# Production Container Images

`deploy/Dockerfile` defines separate API and worker runtime targets. Both run as
the fixed non-root identity `10001:10001`, emit JSON logs by default, use exec-form
entrypoints, and accept `SIGTERM` for graceful shutdown. The API image includes
the matching SSR assets and checks database-and-schema readiness.

Build release artifacts and both images with:

```bash
scripts/build-web.sh
WAREBOXES_IMAGE_PREFIX=registry.example/wareboxes \
WAREBOXES_IMAGE_TAG="$(git rev-parse HEAD)" \
WAREBOXES_IMAGE_SOURCE=https://example.invalid/wareboxes \
scripts/build-images.sh
```

The build script uses a minimal temporary context, validates image and tag names,
adds OCI source/revision labels, and verifies the non-root user and API healthcheck.
Pass database URLs, publisher credentials, and other secrets through the target
orchestrator's secret facility; never bake them into an image or image layer.

The existing versioned systemd deployment remains the baseline single-host
production-like environment. These images are the portable runtime artifacts for
an orchestrated deployment definition; promotion must identify them by immutable
registry digest, not a mutable tag.
