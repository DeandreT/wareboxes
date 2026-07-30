# Wareboxes RF

Native Android RF client built with egui/eframe and Android `NativeActivity`.
It is a member of the Wareboxes Rust workspace and ships as a separate APK from
the web operations application.

## Command Durability

Operator commands are stored in an app-private SQLite database before they can
enter network dispatch. Each record retains its tenant, operator, and device
scope; idempotency identity; exact versioned API path and body; and a body hash.
Every retry keeps those immutable request bytes while creating a distinct
transport attempt and request ID. An interrupted in-flight attempt reopens as
ambiguous and requires an exact replay or reconciliation.

## Authentication and Transport

The app creates an RF-specific versioned session and holds its opaque bearer
token only in memory. The app-private database retains a random device identity
and validated server URL, but never credentials or session tokens. After sign-in,
the operator, tenant, and device identities form the execution scope used for
every durable command.

Authenticated requests send the tenant, request, and idempotency headers required
by the public API. A server response is stored before it changes the workflow.
Known transient responses remain retryable with the original command bytes;
malformed successful responses and indeterminate business conflicts stop work for
reconciliation. On startup and after sign-in, the app recovers unresolved device
commands before requesting the operator's current putaway claim.

Active putaway claims are verified immediately and renewed through the versioned
heartbeat command. Heartbeat retries keep one idempotency identity, server lease
windows are anchored to a monotonic device clock, and late callbacks cannot extend
ownership. Scan and release actions remain blocked until the claim is verified and
stop again before the last confirmed lease becomes unsafe.

## Build Requirements

- Rust 1.92 or newer.
- `cargo-apk`.
- The `aarch64-linux-android` Rust target.
- A current Android SDK and build tools.
- Android NDK with `ANDROID_NDK_ROOT` set.
- An arm64 Android device for `cargo apk run`.

Build an APK from the repository root:

```sh
scripts/build-rf-android.sh
```

Install and run it on a connected arm64 device:

```sh
scripts/install-rf-android.sh
```

For a host-side compile check of the library:

```sh
cargo check --manifest-path apps/rf-android/Cargo.toml --lib
```

Run the same RF UI in a desktop-sized handheld preview:

```sh
cargo run -p wareboxes-rf-android --example rf_preview
```

The generated manifest uses `android.app.NativeActivity`. `cargo-apk` adds the
native library metadata and launcher intent filter from the `cdylib` artifact.
The package metadata declares API 26 as the minimum, API 36 as the target,
portrait orientation, a no-action-bar theme, HTTPS-only networking, and the
`INTERNET` permission.
