# Native Android app (Compose + miuix)

Plan for replacing the Tauri/React UI on Android with a native Jetpack Compose
UI built on [miuix](https://github.com/compose-miuix-ui/miuix), backed by the
existing Rust code extracted into a Tauri-free library. Desktop keeps using
Tauri and the React frontend.

This file is the contract between the three work streams (A: Rust core,
B: Android UI, C: bridge and platform glue). Change it when the contract
changes, in the same commit.

## Decisions

- Native Compose UI with miuix 0.9.4 (`top.yukonga.miuix.kmp:miuix-ui` and
  friends). Kotlin 2.4.21 + Compose Multiplatform 1.12.2 (what miuix is built
  with). The Android app is a standalone Gradle project in `android/`; it no
  longer goes through `tauri android`.
- The backend becomes a Tauri-free crate, `core/` (`iloader-core`). `src-tauri`
  turns into a thin adapter over it; desktop behaviour must not change.
- Rust is exposed to Kotlin with UniFFI (proc macros, library mode), in a new
  crate `android/bridge` (cdylib). Kotlin gets `suspend` functions.
- Languages: Chinese (`zh-CN`) and English only for now.
- Scope: full feature parity with the React UI.
- No real iPhone is available during development. USB, pairing and sideloading
  cannot be verified end to end. State this plainly in every report.

## Repository layout

```
core/                 iloader-core: account, device, sideload, pairing,
                      operation, secure storage, errors, usbmux (moved from
                      src-tauri/src/usbmux). No tauri dependency.
src-tauri/            Desktop adapter: #[tauri::command] wrappers around Core,
                      events forwarded to the webview. Android specific code
                      here (gen/android, usbmux::android plugin) is removed
                      once stream C is done.
src/                  React frontend (desktop only from now on).
android/
  app/                Compose app (Kotlin): theme, navigation, screens.
  bridge/             Rust crate with UniFFI exports (cdylib), depends on core.
  gradle/, build.gradle.kts, settings.gradle.kts
docs/android-native.md
```

A Cargo workspace at the repository root contains `core`, `src-tauri` and
`android/bridge`.

## Rust core API (`iloader-core`)

Everything the UI needs is a method on `Core`. Types are the existing ones
(`DeviceInfo`, `CertificateInfo`, `ListAppIdsResponse`, `PairingAppInfo`,
`AppError`) moved without changing their serde shape, because the React
frontend still consumes them.

```rust
pub struct Core { .. }

pub trait Host: Send + Sync {
    /// App private directory (stores, logs, usbmux socket, caches).
    fn data_dir(&self) -> PathBuf;
    /// Secret storage for saved Apple ID passwords and the anisette state.
    /// Desktop: the OS keyring. Android: Keystore backed, implemented in Kotlin.
    fn secrets(&self) -> Option<Arc<dyn SecretStore>>;
    /// Called from any thread; must not block.
    fn emit(&self, event: CoreEvent);
}

impl Core {
    pub fn new(host: Arc<dyn Host>) -> Arc<Core>;

    // account
    pub async fn login_new(&self, email: String, password: String,
                           anisette_server: String, save_credentials: bool) -> Result<(), AppError>;
    pub async fn login_stored(&self, email: String, anisette_server: String) -> Result<(), AppError>;
    pub fn logged_in_as(&self) -> Option<String>;
    pub fn invalidate_account(&self);
    pub fn stored_accounts(&self) -> Vec<String>;            // was store "ids" in data.json
    pub fn delete_account(&self, email: String) -> Result<(), AppError>;
    pub fn submit_two_factor(&self, code: String);           // answer to CoreEvent::TwoFactorRequired
    pub fn respond_max_certs(&self, serials: Option<Vec<String>>); // answer to MaxCertsReached; None = cancel
    pub async fn certificates(&self) -> Result<Vec<CertificateInfo>, AppError>;
    pub async fn revoke_certificate(&self, serial_number: String) -> Result<(), AppError>;
    pub async fn list_app_ids(&self) -> Result<ListAppIdsResponse, AppError>;
    pub async fn delete_app_id(&self, app_id_id: String) -> Result<(), AppError>;

    // devices
    pub async fn list_devices(&self) -> Result<Vec<Result<DeviceInfo, AppError>>, AppError>;
    pub async fn select_device(&self, device: Option<DeviceInfo>) -> Result<(), AppError>;
    pub fn cancel_pairing(&self);

    // operations: progress arrives as CoreEvent::Operation
    pub async fn sideload(&self, app_path: String) -> Result<(), AppError>;
    pub async fn install_sidestore(&self, nightly: bool, live_container: bool) -> Result<(), AppError>;

    // pairing files
    pub async fn installed_pairing_apps(&self) -> Result<Vec<PairingAppInfo>, AppError>;
    pub async fn place_pairing(&self, bundle_id: String, path: String) -> Result<(), AppError>;
    pub async fn export_pairing(&self) -> Result<PairingExport, AppError>; // bytes + suggested file name; the UI saves it
    pub async fn has_stored_rppairing(&self, device: DeviceInfo) -> Result<bool, AppError>;
    pub async fn delete_stored_rppairing(&self) -> Result<(), AppError>;

    // settings related
    pub fn keyring_available(&self) -> bool;
    pub fn force_disable_keyring(&self, force: bool);
    pub fn reset_anisette_state(&self) -> Result<bool, AppError>;
}
```

`login_*` need the 2FA code and the "too many certificates" decision from the
user. They stay as they are today (event out, answer back, 120 s / 300 s
timeout) but without Tauri:

```rust
pub enum CoreEvent {
    TwoFactorRequired(TwoFactorParams),            // was "2fa-required"
    MaxCertsReached(Vec<CertificateInfo>),         // was "max-certs-reached"
    Operation { operation_id: String, update: OperationUpdate }, // was "operation_<id>"
    Log(LogRecord),                                // was "log-record"
    UsbPairing(PairEvent),                         // was "usbmux-pairing"
}
pub struct OperationUpdate { update_type: Started|Finished|Failed, step_id: String, extra_details: Option<AppError> }
```

Operation ids and steps (UI side, as in `src/components/operations.ts`):

| operation id        | steps                              |
| ------------------- | ---------------------------------- |
| `sideload`          | `install`                          |
| `install_sidestore` | `download`, `install`, `pairing`   |

(The LiveContainer variant uses the id `install_sidestore` too, with
`live_container = true`; only the titles differ.)

`AppError` serialises as `{ "type": "<snake_case variant>", "message": "..." }`.
The variant to suggestion mapping lives in `src/errors.tsx`
(`errorSuggestionKeys`); the Kotlin UI reimplements it.

### Things that currently depend on Tauri and must be abstracted

- `tauri_plugin_store` (`data.json` for stored account ids, pairing storage
  entries, host labels): replace with a small JSON key-value store in `core`
  (file in `data_dir`).
- `Window`/`AppHandle` emit/listen for 2FA, max certs, operation progress and
  logs: replaced by `Host::emit` and the `submit_*`/`respond_*` methods.
- `tauri_plugin_dialog` in `export_pairing_cmd` / `place_pairing_cmd`: the core
  returns bytes or takes a path; picking files is the UI's job.
- `keyring` direct use in `account.rs` and `secure_storage.rs`: go through
  `SecretStore`.
- `logging.rs`: the tracing layer forwards to `Host::emit(CoreEvent::Log)`.

## Kotlin side

The UI only knows an interface, so it can be built against a fake first:

```kotlin
interface IloaderCore {
    val events: SharedFlow<CoreEvent>
    // account
    suspend fun loginNew(email: String, password: String, anisetteServer: String, saveCredentials: Boolean)
    suspend fun loginStored(email: String, anisetteServer: String)
    fun loggedInAs(): String?
    fun invalidateAccount()
    fun storedAccounts(): List<String>
    fun deleteAccount(email: String)
    fun submitTwoFactor(code: String)
    fun respondMaxCerts(serials: List<String>?)
    suspend fun certificates(): List<CertificateInfo>
    suspend fun revokeCertificate(serialNumber: String)
    suspend fun listAppIds(): ListAppIdsResponse
    suspend fun deleteAppId(appIdId: String)
    // devices
    suspend fun listDevices(): List<DeviceResult>      // Ok(DeviceInfo) | Err(AppError)
    suspend fun selectDevice(device: DeviceInfo?)
    fun cancelPairing()
    // operations
    suspend fun sideload(appPath: String)
    suspend fun installSidestore(nightly: Boolean, liveContainer: Boolean)
    // pairing files
    suspend fun installedPairingApps(): List<PairingAppInfo>
    suspend fun placePairing(bundleId: String, path: String)
    suspend fun exportPairing(): PairingExport
    suspend fun hasStoredRppairing(device: DeviceInfo): Boolean
    suspend fun deleteStoredRppairing()
    // settings related
    fun keyringAvailable(): Boolean
    fun forceDisableKeyring(force: Boolean)
    suspend fun resetAnisetteState(): Boolean
}
```

Failures are thrown as `IloaderException(type: String, message: String)`
(the `AppError` shape). `CoreEvent` mirrors the Rust enum (sealed class).

Kotlin owned services (not in `IloaderCore`):

- User preferences (DataStore): `anisetteServer` (default `ani.jkcoxson.com`),
  `overrideKeyring` (false), `lang` (system default), saved selection.
- `SecretStore` implementation backed by Android Keystore (AES-GCM, secrets in
  private storage). Handed to Rust through a UniFFI foreign trait.
- USB host: Kotlin discovers and opens the iPhone, then calls
  `core.attachUsbDevice(fd, serial, productId, epIn, epOut, maxPacketOut, name)`
  and `core.detachUsbDevice(name)`. This replaces the Tauri `UsbPlugin`
  polling; it uses `UsbManager` broadcasts instead. The logic in
  `src-tauri/gen/android/.../UsbPlugin.kt` (interface and configuration
  selection, permission) moves over as is.
- File access: `OpenDocument` for IPA files (copied to the cache directory and
  passed as `appPath`), `CreateDocument` for exporting the pairing file.
- Opening links: `Intent.ACTION_VIEW`.

## Screens (parity with the React UI)

| Screen / element | Backend | React source |
| --- | --- | --- |
| Header: logo, version, Donate and GitHub links | - | `App.tsx` |
| Apple ID card: login form (email, password, save credentials), logged in state with sign out, saved accounts (log in / delete) | `loginNew`, `loginStored`, `loggedInAs`, `invalidateAccount`, `storedAccounts`, `deleteAccount` | `AppleID.tsx` |
| 2FA code dialog | `TwoFactorRequired` / `submitTwoFactor` | `AppleID.tsx` |
| "Too many certificates" dialog (pick certificates to revoke) | `MaxCertsReached` / `respondMaxCerts` | `AppleID.tsx` |
| Management list: pairing files, refresh devices, certificates, App IDs | - | `App.tsx` |
| Device card: list, select, refresh, pairing in progress with cancel, iPhone "Trust" prompt state | `listDevices`, `selectDevice`, `cancelPairing`, `UsbPairing` events | `Device.tsx` |
| Installers: SideStore stable/nightly, LiveContainer + SideStore stable/nightly, import IPA | `installSidestore`, `sideload` | `App.tsx` |
| Operation view: step list (started / finished / failed), success message, links | `Operation` events | `OperationView.tsx` |
| Certificates page: list, revoke | `certificates`, `revokeCertificate` | `pages/Certificates.tsx` |
| App IDs page: list with quota, delete | `listAppIds`, `deleteAppId` | `pages/AppIds.tsx` |
| Pairing page: installed apps, place pairing file, export | `installedPairingApps`, `placePairing`, `exportPairing` | `pages/Pairing.tsx` |
| Settings: anisette server, keyring override, reset anisette, delete stored pairing, language, log viewer, copy logs | `forceDisableKeyring`, `resetAnisetteState`, `deleteStoredRppairing`, `Log` events | `pages/Settings.tsx`, `LogContext.tsx` |
| Error dialog: message, per variant suggestions, copy | - | `ErrorContext.tsx`, `errors.tsx` |

Strings come from `src/locales/en.json` and `zh_cn.json` (about 290 keys,
`{{name}}` placeholders). Convert them to Android `strings.xml` with a script
(`android/tools/convert_locales.py`) so they can be regenerated; `%1$s`
placeholders, plurals not needed.

## Work streams

A. **Rust core** (files: `core/`, `src-tauri/`, root `Cargo.toml`). Extract
   the backend as described above. `cargo test`, `cargo check` for desktop and
   `aarch64-linux-android` stay green; the existing 93 usbmux tests move with
   the module. Desktop commands and events keep their names and shapes.

B. **Android UI** (files: `android/app`, `android/gradle`, root Gradle files,
   `android/tools`). Gradle project on Kotlin 2.4.21 with miuix, theme
   (`ThemeController`, Monet + dark), navigation, every screen above, i18n
   conversion, the `IloaderCore` interface and a `FakeCore` with believable
   data and timings (including operation progress, 2FA, max certificates and
   errors) so everything can be exercised on an emulator without Rust. No
   dependency on streams A or C.

C. **Bridge and platform glue** (files: `android/bridge`, Kotlin platform
   classes in `android/app/.../platform`). After A and B: UniFFI crate,
   `RealCore : IloaderCore`, Keystore `SecretStore`, USB host, file access,
   build integration (cargo-ndk or a Gradle task), removal of the Tauri
   Android pieces.
