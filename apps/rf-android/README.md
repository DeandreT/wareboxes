# Wareboxes RF

Native Android RF client built with egui/eframe and Android `NativeActivity`.
It is a member of the Wareboxes Rust workspace and ships as a separate APK from
the operations client.

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
