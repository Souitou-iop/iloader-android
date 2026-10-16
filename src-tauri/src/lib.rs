mod host;

use std::sync::Arc;

use iloader_core::{
    AppError, CertificateInfo, Core, DeviceInfo, ListAppIdsResponse, PairingAppInfo,
};
use tauri::{Listener, Manager, State};
use tauri_plugin_dialog::DialogExt;

type CoreState<'a> = State<'a, Arc<Core>>;

#[tauri::command]
async fn login_new(
    core: CoreState<'_>,
    email: String,
    password: String,
    anisette_server: String,
    save_credentials: bool,
) -> Result<(), AppError> {
    core.login_new(email, password, anisette_server, save_credentials)
        .await
}

#[tauri::command]
async fn login_stored(
    core: CoreState<'_>,
    email: String,
    anisette_server: String,
) -> Result<(), AppError> {
    core.login_stored(email, anisette_server).await
}

#[tauri::command]
fn delete_account(core: CoreState<'_>, email: String) -> Result<(), AppError> {
    core.delete_account(email)
}

#[tauri::command]
fn logged_in_as(core: CoreState<'_>) -> Option<String> {
    core.logged_in_as()
}

#[tauri::command]
fn invalidate_account(core: CoreState<'_>) {
    core.invalidate_account()
}

#[tauri::command]
fn reset_anisette_state(core: CoreState<'_>) -> Result<bool, AppError> {
    core.reset_anisette_state()
}

#[tauri::command]
async fn get_certificates(core: CoreState<'_>) -> Result<Vec<CertificateInfo>, AppError> {
    core.certificates().await
}

#[tauri::command]
async fn revoke_certificate(core: CoreState<'_>, serial_number: String) -> Result<(), AppError> {
    core.revoke_certificate(serial_number).await
}

#[tauri::command]
async fn list_app_ids(core: CoreState<'_>) -> Result<ListAppIdsResponse, AppError> {
    core.list_app_ids().await
}

#[tauri::command]
async fn delete_app_id(core: CoreState<'_>, app_id_id: String) -> Result<(), AppError> {
    core.delete_app_id(app_id_id).await
}

#[tauri::command]
async fn list_devices(core: CoreState<'_>) -> Result<Vec<Result<DeviceInfo, AppError>>, AppError> {
    core.list_devices().await
}

#[tauri::command]
async fn set_selected_device(
    core: CoreState<'_>,
    device: Option<DeviceInfo>,
) -> Result<(), AppError> {
    core.select_device(device).await
}

#[tauri::command]
async fn cancel_pairing(core: CoreState<'_>) -> Result<(), AppError> {
    core.cancel_pairing();
    Ok(())
}

#[tauri::command]
async fn sideload_operation(core: CoreState<'_>, app_path: String) -> Result<(), AppError> {
    core.sideload(app_path).await
}

#[tauri::command]
async fn install_sidestore_operation(
    core: CoreState<'_>,
    nightly: bool,
    live_container: bool,
) -> Result<(), AppError> {
    core.install_sidestore(nightly, live_container).await
}

#[tauri::command]
async fn installed_pairing_apps(core: CoreState<'_>) -> Result<Vec<PairingAppInfo>, AppError> {
    core.installed_pairing_apps().await
}

#[tauri::command]
async fn place_pairing_cmd(
    core: CoreState<'_>,
    bundle_id: String,
    path: String,
) -> Result<(), AppError> {
    core.place_pairing(bundle_id, path).await
}

// Prompt for a location to save the pairing file, then export it there.
#[tauri::command]
async fn export_pairing_cmd(core: CoreState<'_>, app: tauri::AppHandle) -> Result<(), AppError> {
    let export = core.export_pairing().await?;

    let save_path = app
        .dialog()
        .file()
        .add_filter("Pairing File", &["plist", "mobiledevicepairing"])
        .set_file_name(&export.file_name)
        .set_title("Export Pairing File")
        .blocking_save_file();

    if let Some(save_path) = save_path
        && let Some(save_path) = save_path.as_path()
    {
        tokio::fs::write(save_path, &export.bytes)
            .await
            .map_err(|e| {
                AppError::Filesystem("Failed to write pairing file".into(), e.to_string())
            })?;

        Ok(())
    } else {
        Err(AppError::Canceled("Export".into()))
    }
}

#[tauri::command]
async fn delete_stored_rppairing(core: CoreState<'_>) -> Result<(), AppError> {
    core.delete_stored_rppairing().await
}

#[tauri::command]
async fn has_stored_rppairing(core: CoreState<'_>, device: DeviceInfo) -> Result<bool, AppError> {
    core.has_stored_rppairing(device).await
}

#[tauri::command]
fn keyring_available(core: CoreState<'_>) -> bool {
    core.keyring_available()
}

#[tauri::command]
fn force_disable_keyring(core: CoreState<'_>, force: bool) {
    core.force_disable_keyring(force)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = tauri::Builder::default().plugin(tauri_plugin_process::init());
    #[cfg(desktop)]
    let builder = builder.plugin(tauri_plugin_updater::Builder::new().build());
    builder
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_store::Builder::new().build())
        .setup(|app| {
            let core = Core::new(Arc::new(host::TauriHost::new(app.handle().clone())));

            // The webview answers 2FA and certificate prompts with events.
            let answers = core.clone();
            app.listen("2fa-recieved", move |event| {
                answers.submit_two_factor(event.payload().trim_matches('"').to_string());
            });
            let answers = core.clone();
            app.listen("max-certs-response", move |event| {
                let serials =
                    serde_json::from_str::<Option<Vec<String>>>(event.payload()).unwrap_or(None);
                answers.respond_max_certs(serials);
            });

            app.manage(core);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            login_new,
            invalidate_account,
            logged_in_as,
            login_stored,
            delete_account,
            list_devices,
            sideload_operation,
            set_selected_device,
            install_sidestore_operation,
            get_certificates,
            revoke_certificate,
            list_app_ids,
            delete_app_id,
            installed_pairing_apps,
            place_pairing_cmd,
            reset_anisette_state,
            export_pairing_cmd,
            delete_stored_rppairing,
            keyring_available,
            force_disable_keyring,
            cancel_pairing,
            has_stored_rppairing,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
