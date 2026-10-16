use futures::FutureExt;
use isideload::{
    anisette::remote_v3::RemoteV3AnisetteProvider,
    auth::apple_account::{AppleAccount, TwoFactorCallbackParams, TwoFactorCallbackResponse},
    dev::{
        app_ids::{AppIdsApi, ListAppIdsResponse},
        certificates::{CertificatesApi, DevelopmentCertificate},
        developer_session::DeveloperSession,
    },
    sideload::{SideloaderBuilder, builder::MaxCertsBehavior, sideloader::Sideloader},
    util::callbacks::MaxCertsCallbackBox,
};
use rootcause::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::Duration;
use tokio::sync::oneshot;
use tracing::debug;

use crate::{Core, CoreEvent, error::AppError, sideload::SideloaderGuard};

const TWO_FACTOR_TIMEOUT: Duration = Duration::from_secs(120);
const MAX_CERTS_TIMEOUT: Duration = Duration::from_secs(300);
const IDS_KEY: &str = "ids";

impl Core {
    pub async fn login_new(
        &self,
        email: String,
        password: String,
        anisette_server: String,
        save_credentials: bool,
    ) -> Result<(), AppError> {
        let account = self.login(&email, &password, anisette_server).await?;
        *self.sideloader.lock().unwrap() = Some(account);

        if save_credentials {
            let secrets = self
                .host
                .secrets()
                .ok_or_else(|| AppError::Keyring("No secret storage available".into()))?;
            secrets.set(&email, &password).map_err(|e| {
                AppError::KeyringWithMessage("Failed to save credentials".into(), e)
            })?;
            let mut existing_ids = self.stored_ids();
            let value = Value::String(email.clone());
            if !existing_ids.contains(&value) {
                existing_ids.push(value);
            }
            self.save_ids(existing_ids)?;
        }
        Ok(())
    }

    pub async fn login_stored(
        &self,
        email: String,
        anisette_server: String,
    ) -> Result<(), AppError> {
        let secrets = self
            .host
            .secrets()
            .ok_or_else(|| AppError::Keyring("No secret storage available".into()))?;
        let password = secrets
            .get(&email)
            .map_err(|e| AppError::KeyringWithMessage("Failed to get credentials".into(), e))?
            .ok_or_else(|| {
                AppError::KeyringWithMessage(
                    "Failed to get credentials".into(),
                    "No matching entry found in secure storage".into(),
                )
            })?;
        let account = self.login(&email, &password, anisette_server).await?;
        *self.sideloader.lock().unwrap() = Some(account);

        Ok(())
    }

    pub fn stored_accounts(&self) -> Vec<String> {
        self.stored_ids()
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect()
    }

    fn stored_ids(&self) -> Vec<Value> {
        self.kv
            .get(IDS_KEY)
            .and_then(|v| v.as_array().cloned())
            .unwrap_or_default()
    }

    fn save_ids(&self, ids: Vec<Value>) -> Result<(), AppError> {
        self.kv
            .set(IDS_KEY, Value::Array(ids))
            .map_err(|e| AppError::Misc(format!("Failed to save store: {e}")))
    }

    pub fn delete_account(&self, email: String) -> Result<(), AppError> {
        let mut existing_ids = self.stored_ids();
        existing_ids.retain(|v| v.as_str().is_none_or(|s| s != email));
        self.save_ids(existing_ids)?;
        let secrets = self
            .host
            .secrets()
            .ok_or_else(|| AppError::Keyring("No secret storage available".into()))?;
        secrets
            .delete(&email)
            .map_err(|e| AppError::KeyringWithMessage("Failed to delete credentials".into(), e))?;
        Ok(())
    }

    pub fn logged_in_as(&self) -> Option<String> {
        let sideloader_guard = self.sideloader.lock().unwrap();
        sideloader_guard
            .as_ref()
            .map(|account| account.get_email().to_string())
    }

    pub fn invalidate_account(&self) {
        *self.sideloader.lock().unwrap() = None;
    }

    pub fn reset_anisette_state(&self) -> Result<bool, AppError> {
        let secrets = self
            .host
            .secrets()
            .ok_or_else(|| AppError::Keyring("No secret storage available".into()))?;
        match secrets.delete("anisette_state") {
            Ok(true) => {
                debug!("Anisette state deleted from keyring.");
                Ok(true)
            }
            Ok(false) => {
                debug!("No existing anisette state found in keyring, nothing to delete.");
                Ok(false)
            }
            Err(e) => Err(AppError::KeyringWithMessage(
                "Failed to delete anisette state".into(),
                e,
            )),
        }
    }

    async fn login(
        &self,
        email: &str,
        password: &str,
        anisette_server: String,
    ) -> Result<Sideloader<MaxCertsCallbackBox>, AppError> {
        let tfa_closure = {
            let core = self.this.clone();
            move |params: TwoFactorCallbackParams| {
                let core = core.clone();

                async move {
                    let core = core
                        .upgrade()
                        .ok_or_else(|| report!("iloader core was shut down"))?;
                    let (tx, rx) = oneshot::channel::<String>();
                    *core.pending_two_factor.lock().unwrap() = Some(tx);
                    core.host.emit(CoreEvent::TwoFactorRequired(params));

                    let result = tokio::time::timeout(TWO_FACTOR_TIMEOUT, rx).await;
                    core.pending_two_factor.lock().unwrap().take();
                    let code = result
                        .map_err(|_| report!("Timed out waiting for the 2FA code"))?
                        .map_err(|_| report!("2FA request was dropped"))?;
                    Ok(TwoFactorCallbackResponse::SubmitCode(code))
                }
                .boxed()
            }
        };

        let anisette_url = if !anisette_server.starts_with("http") {
            format!("https://{}", anisette_server)
        } else {
            anisette_server
        };

        let mut account = AppleAccount::builder(&email.to_lowercase())
            .anisette_provider(
                RemoteV3AnisetteProvider::default()?
                    .set_serial_number("0".to_string())
                    .set_storage(self.create_sideloading_storage()?)
                    .set_url(&anisette_url),
            )
            .login(password, Box::new(tfa_closure))
            .await?;

        debug!("Logged in");

        let dev_session = DeveloperSession::from_account(&mut account).await?;

        debug!("Created developer session");

        let core = self.this.clone();
        let max_certs_callback: MaxCertsCallbackBox =
            Box::new(move |certs: Vec<DevelopmentCertificate>| {
                let core = core.clone();
                Box::pin(async move {
                    let core = core
                        .upgrade()
                        .ok_or_else(|| report!("iloader core was shut down"))?;
                    let cert_infos: Vec<CertificateInfo> = certs
                        .iter()
                        .map(|cert| CertificateInfo {
                            name: cert.name.clone(),
                            certificate_id: cert.certificate_id.clone(),
                            serial_number: cert.serial_number.clone(),
                            machine_name: cert.machine_name.clone(),
                            machine_id: cert.machine_id.clone(),
                        })
                        .collect();
                    let (tx, rx) = oneshot::channel::<Option<Vec<String>>>();
                    *core.pending_max_certs.lock().unwrap() = Some(tx);
                    core.host.emit(CoreEvent::MaxCertsReached(cert_infos));

                    let result = tokio::time::timeout(MAX_CERTS_TIMEOUT, rx).await;
                    core.pending_max_certs.lock().unwrap().take();
                    Ok(result
                        .map_err(|_| report!("Timed out waiting for the certificate choice"))?
                        .map_err(|_| report!("Certificate request was dropped"))?)
                })
            });

        // TODO: Team Selection

        let sideloader = SideloaderBuilder::new(dev_session, email.to_lowercase())
            .machine_name("iloader".into())
            .storage(self.create_sideloading_storage()?)
            .max_certs_behavior(MaxCertsBehavior::Prompt(max_certs_callback))
            .build();

        debug!("Built sideloader");

        Ok(sideloader)
    }

    pub async fn certificates(&self) -> Result<Vec<CertificateInfo>, AppError> {
        let mut sideloader = SideloaderGuard::take(&self.sideloader)?;

        let team = sideloader.get_mut().get_team().await?;
        let dev_session = sideloader.get_mut().get_dev_session();

        let certificates = dev_session.list_all_development_certs(&team, None).await?;

        Ok(certificates
            .into_iter()
            .map(|cert| CertificateInfo {
                name: cert.name,
                certificate_id: cert.certificate_id,
                serial_number: cert.serial_number,
                machine_name: cert.machine_name,
                machine_id: cert.machine_id,
            })
            .collect())
    }

    pub async fn revoke_certificate(&self, serial_number: String) -> Result<(), AppError> {
        let mut sideloader = SideloaderGuard::take(&self.sideloader)?;

        let team = sideloader.get_mut().get_team().await?;
        let dev_session = sideloader.get_mut().get_dev_session();

        dev_session
            .revoke_development_cert(&team, &serial_number, None)
            .await?;

        Ok(())
    }

    pub async fn list_app_ids(&self) -> Result<ListAppIdsResponse, AppError> {
        let mut sideloader = SideloaderGuard::take(&self.sideloader)?;

        let team = sideloader.get_mut().get_team().await?;
        let dev_session = sideloader.get_mut().get_dev_session();

        let response = dev_session.list_app_ids(&team, None).await?;

        Ok(response.clone())
    }

    pub async fn delete_app_id(&self, app_id_id: String) -> Result<(), AppError> {
        let mut sideloader = SideloaderGuard::take(&self.sideloader)?;

        let team = sideloader.get_mut().get_team().await?;
        let dev_session = sideloader.get_mut().get_dev_session();

        dev_session.delete_app_id(&team, &app_id_id, None).await?;

        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CertificateInfo {
    pub name: Option<String>,
    pub certificate_id: Option<String>,
    pub serial_number: Option<String>,
    pub machine_name: Option<String>,
    pub machine_id: Option<String>,
}
