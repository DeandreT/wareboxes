# Versioning

The major API version is part of every public path:

```text
/api/v1/...
```

Within a major version, Wareboxes may add endpoints, optional request fields,
response fields, enum values, error reasons, and webhook event types. Integrations
must ignore response fields they do not recognize and handle unknown nonterminal
status or reason values safely.

Wareboxes will not silently change the meaning of an existing required field,
remove a field or operation, or make an optional request field required within v1.
A breaking contract requires a new major version and a documented migration path.

The OpenAPI `info.version` identifies the published contract revision. The URL major
version changes only for incompatible API generations, not for every application
release.
