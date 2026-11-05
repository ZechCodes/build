//! Authenticated metadata reads use the shared bounded SQL executor.
use super::{AppState, DeferredWork};
use crate::api::v1::secrets::{AuditPage, AuditParams, ListParams, MetadataPage};
use crate::api::v1::{answer, Answer};
use crate::api::{self, ApiError};
use crate::execution::CallContext;
use crate::secrets::SecretError;
use crate::store::{ReadConnection, StoreWriter};
use crate::transport::Frame;
use serde_json::Value;
use std::sync::Arc;

pub(in crate::app) struct SecretReadService {
    device: Option<String>,
    admission: Option<Arc<crate::update::AdmissionGate>>,
}

impl AppState {
    pub(in crate::app) fn secret_read_service(&self) -> SecretReadService {
        SecretReadService {
            device: self
                .secret_service()
                .map(|service| service.device_id().to_owned()),
            admission: self.update_admission(),
        }
    }
    pub(crate) fn secret_list(
        &mut self,
        context: Option<&CallContext>,
        params: ListParams,
    ) -> Result<Answer<MetadataPage>, ApiError> {
        let context = context.ok_or_else(|| ApiError::unavailable("unauthorized"))?;
        let (device, creator) = self
            .secret_user_context(context)
            .map_err(ApiError::classify)?;
        let context = context.clone();
        self.authorize_secret_scope(&params.scope)?;
        let writer = self.secret_metadata_writer()?;
        self.deferred_work = Some(DeferredWork::External(Box::new(move || {
            if !context.execution_is_live() {
                return Err("unauthorized".into());
            }
            writer
                .secret_metadata_page(&device, &creator, &params)
                .map_err(read_error)
                .and_then(to_value)
                .map_err(|error| error.message().to_owned())
        })));
        answer(Ok(Value::Null))
    }
    pub(crate) fn secret_audit(
        &mut self,
        context: Option<&CallContext>,
        params: AuditParams,
    ) -> Result<Answer<AuditPage>, ApiError> {
        let context = context.ok_or_else(|| ApiError::unavailable("unauthorized"))?;
        let (device, creator) = self
            .secret_user_context(context)
            .map_err(ApiError::classify)?;
        let context = context.clone();
        self.authorize_secret_scope(&params.scope)?;
        let writer = self.secret_metadata_writer()?;
        self.deferred_work = Some(DeferredWork::External(Box::new(move || {
            if !context.execution_is_live() {
                return Err("unauthorized".into());
            }
            writer
                .secret_metadata_audit(&device, &creator, &params)
                .map_err(read_error)
                .and_then(to_value)
                .map_err(|error| error.message().to_owned())
        })));
        answer(Ok(Value::Null))
    }
    fn secret_metadata_writer(&self) -> Result<StoreWriter, ApiError> {
        self.store
            .as_ref()
            .map(|store| store.writer())
            .ok_or_else(|| ApiError::unavailable("secret_unavailable"))
    }
}

fn to_value<T: serde::Serialize>(value: T) -> Result<Value, ApiError> {
    serde_json::to_value(value).map_err(|_| ApiError::internal("secret metadata serialization"))
}
fn read_error(error: SecretError) -> ApiError {
    match error {
        SecretError::Invalid => ApiError::invalid_params("secret_invalid"),
        _ => ApiError::unavailable("secret_unavailable"),
    }
}

impl SecretReadService {
    pub(in crate::app) fn call(
        &self,
        sender: &CallContext,
        frame: Frame,
        mut connection: Option<ReadConnection>,
    ) -> Value {
        let id = frame.payload.get("id").cloned().unwrap_or(Value::Null);
        let result = self.read(sender, &frame.payload, connection.as_mut());
        api::reply(id, result)
    }
    fn read(
        &self,
        sender: &CallContext,
        payload: &Value,
        connection: Option<&mut ReadConnection>,
    ) -> Result<Value, ApiError> {
        if !sender.read_is_live() {
            return Err(ApiError::unavailable("request authorization has ended"));
        }
        let device = self
            .device
            .as_deref()
            .ok_or_else(|| ApiError::unavailable("secret_unavailable"))?;
        let _lease = self
            .admission
            .as_ref()
            .map(|gate| {
                gate.try_enter_read()
                    .ok_or_else(|| ApiError::busy("bridge update handoff in progress"))
            })
            .transpose()?;
        let method = payload["method"].as_str().unwrap_or("");
        let params = api::v1::normalize_read_params(method, &payload["params"])?;
        let connection = connection.ok_or_else(|| ApiError::unavailable("secret_unavailable"))?;
        let result = connection
            .read(|read| match method {
                "secrets.list" => {
                    let params = api::v1::parse_params::<ListParams>(&params)?;
                    read.secret_metadata_page(device, "device_user", &params)
                        .map_err(read_error)
                        .and_then(to_value)
                }
                "secrets.audit" => {
                    let params = api::v1::parse_params::<AuditParams>(&params)?;
                    read.secret_metadata_audit(device, "device_user", &params)
                        .map_err(read_error)
                        .and_then(to_value)
                }
                _ => Err(ApiError::unknown_method(method)),
            })
            .map_err(|_| ApiError::unavailable("secret_unavailable"))??;
        if !sender.read_is_live() {
            return Err(ApiError::unavailable("request authorization has ended"));
        }
        api::v1::check_read_result(method, result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::execution::session::{
        ClientSessions, DeliveryError, EncodedEvent, HttpIdentity, Opening, ReplySink,
        VerifiedCaller,
    };
    use crate::secrets::GrantScope;
    use crate::store::Store;
    use serde_json::json;
    struct Sink;
    impl ReplySink for Sink {
        fn try_send(&self, _: EncodedEvent) -> Result<(), DeliveryError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn http_and_webrtc_read_authority_match_and_retired_contexts_fail_closed() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path()).unwrap();
        store.writer_for_test().execute("INSERT INTO device_project_ids(path,project_id) VALUES ('/synthetic/project','project-a')",[]).unwrap();
        let secret = crate::secrets::SecretRecord {
            context: crate::secrets::SecretContext {
                id: format!("sec_{}", uuid::Uuid::new_v4()),
                device_id: "synthetic-device".into(),
                creator: "device_user".into(),
                name: "SYNTHETIC_TOKEN".into(),
                ttl_seconds: None,
                grant: GrantScope::Project {
                    project_id: "project-a".into(),
                },
                stage: crate::secrets::StageContext {
                    owner_id: "synthetic-owner".into(),
                    conversation_id: "synthetic-conversation".into(),
                    generation: "synthetic-generation".into(),
                    draft_id: None,
                },
            },
            created_at: 1_800_000_000,
            attached_at: None,
            expires_at: None,
            revoked_at: None,
            revocation_reason: None,
            revision: 1,
            message_operation_id: None,
            upload_request_id: uuid::Uuid::new_v4().to_string(),
            upload_digest: "ab".repeat(32),
            blob_present: true,
        };
        store.writer().secrets_stage(&secret).unwrap();
        let service = SecretReadService {
            device: Some("synthetic-device".into()),
            admission: None,
        };
        let sessions = ClientSessions::new("synthetic-epoch");
        let http = VerifiedCaller::verified_http(
            HttpIdentity {
                issuer: "https://synthetic.example".into(),
                account_id: "synthetic-owner".into(),
                device_id: "synthetic-device".into(),
                family_id: "synthetic-family".into(),
                origin: "https://synthetic.example".into(),
                endpoint_epoch: "synthetic-endpoint".into(),
                authorization_epoch: 1,
            },
            tokio::time::Instant::now() + std::time::Duration::from_secs(60),
            Arc::new(|| true),
        );
        let webrtc = VerifiedCaller::verified_relay("synthetic-device", Opening::new());
        let mut accepted = Vec::new();
        for caller in [http, webrtc] {
            let session = sessions.open(caller).unwrap();
            let request = sessions.request(&session, Arc::new(Sink), None).unwrap();
            let context = CallContext::from_request(&request);
            let payload = json!({"method":"secrets.list","params":{"scope":{"project_id":"project-a"},"secret_ids":[secret.context.id]}});
            let mut connection = store.read_pool().acquire().await;
            let result = service
                .read(&context, &payload, Some(&mut connection))
                .unwrap();
            accepted.push(result["secrets"].clone());
            sessions.close(&session);
            assert_eq!(
                service
                    .read(&context, &payload, Some(&mut connection))
                    .err()
                    .unwrap()
                    .code(),
                "unavailable"
            );
        }
        assert_eq!(accepted[0], accepted[1]);
        assert_eq!(accepted[0][0]["secret_id"], secret.context.id);
    }
}
