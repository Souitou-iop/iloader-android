use serde::Serialize;

use crate::{CoreEvent, Host, error::AppError};

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum UpdateType {
    Started,
    Finished,
    Failed,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationUpdate {
    pub update_type: UpdateType,
    pub step_id: String,
    pub extra_details: Option<AppError>,
}

pub struct Operation<'a> {
    id: String,
    host: &'a dyn Host,
}

impl<'a> Operation<'a> {
    pub fn new(id: &str, host: &'a dyn Host) -> Operation<'a> {
        Operation {
            id: id.to_string(),
            host,
        }
    }

    fn send(&self, update_type: UpdateType, step_id: &str, extra_details: Option<AppError>) {
        self.host.emit(CoreEvent::Operation {
            operation_id: self.id.clone(),
            update: OperationUpdate {
                update_type,
                step_id: step_id.to_string(),
                extra_details,
            },
        });
    }

    pub fn move_on(&self, old_id: &str, new_id: &str) {
        self.complete(old_id);
        self.start(new_id);
    }

    pub fn start(&self, id: &str) {
        self.send(UpdateType::Started, id, None);
    }

    pub fn complete(&self, id: &str) {
        self.send(UpdateType::Finished, id, None);
    }

    pub fn fail<T>(&self, id: &str, error: AppError) -> Result<T, AppError> {
        self.send(UpdateType::Failed, id, Some(error.clone()));
        Err(error)
    }

    pub fn fail_if_err<T>(&self, id: &str, res: Result<T, AppError>) -> Result<T, AppError> {
        match res {
            Ok(t) => Ok(t),
            Err(e) => self.fail::<T>(id, e),
        }
    }
}
