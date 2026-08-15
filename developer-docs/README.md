# Wareboxes developer documentation

This directory is the source for the public Scalar developer portal. Public API
schemas and operations originate in Rust; the generated OpenAPI file must not be
edited by hand.

Generate the public contract:

```bash
cargo run -p wareboxes-api --example export_integration_openapi -- \
  developer-docs/api-reference/integrations-v1.openapi.json
```

Check that the committed contract is current:

```bash
cargo run -p wareboxes-api --example export_integration_openapi -- \
  --check developer-docs/api-reference/integrations-v1.openapi.json
```

Scalar CLI 2.x requires Node.js 24 or newer. Preview the portal from this
directory:

```bash
cd developer-docs
npx @scalar/cli project preview
```

The API reference is also available from a running Wareboxes server at
`/openapi/integrations/v1.json`.
