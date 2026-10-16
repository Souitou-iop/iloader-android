# iloader for Android (native)

Jetpack Compose + miuix UI (`app/`) on top of the Rust core (`../core`) through
a UniFFI bridge (`bridge/`). See `../docs/android-native.md` for the design.

## Building

Needs a JDK 21, the Android SDK with NDK `28.2.13676358` (`local.properties`
or `ANDROID_HOME`), Rust with the `aarch64-linux-android`,
`armv7-linux-androideabi` and `x86_64-linux-android` targets, and `cargo` on
`PATH` (or in `$CARGO_HOME/bin`).

```sh
cd android
./gradlew :app:assembleDebug
```

Gradle does everything, no cargo-ndk needed:

1. `buildRustLibs<Variant>` runs `cargo build -p iloader-bridge --target <triple>`
   per ABI with the NDK clang as linker (debug variant: Cargo dev profile with
   debug info stripped, release variant: `--release`) and copies
   `libiloader_bridge.so` into the APK's `jniLibs`.
2. `generateBindings<Variant>` runs the `uniffi-bindgen` binary of the bridge
   crate in library mode on that `.so` and adds the generated Kotlin
   (`io.github.souitouiop.iloader.bridge`) to the app's sources. The bindings
   are generated at build time and not checked in.

Options (Gradle properties):

- `-PrustAbis=arm64-v8a` builds only the listed ABIs (comma separated,
  default `arm64-v8a,armeabi-v7a,x86_64`); much faster for local runs.
- `-PfakeCore=true` runs the UI against `FakeCore` (no Rust, no USB) via
  `BuildConfig.USE_FAKE_CORE`.

## Layout

- `bridge/` Rust crate `iloader-bridge` (cdylib), UniFFI proc macros. Flat
  records/enums, `suspend` functions, `CoreListener` and `BridgeSecretStore`
  foreign traits, `AppError` as `BridgeException.App(kind, detail)`.
- `app/.../core/RealCore.kt` the `IloaderCore` implementation over the bridge.
- `app/.../platform/KeystoreSecretStore.kt` AES-GCM, key in Android Keystore,
  ciphertexts in `files/secrets.json`.
- `app/.../platform/UsbHost.kt` `UsbManager` broadcasts, permission, usbmux
  interface setup, hands the descriptor to `attachUsbDevice`.

Not verified without an iPhone: USB attach, pairing, sideloading.
