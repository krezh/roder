//! Talos power actions (reboot/shutdown) and systemd-style service control
//! (start/stop/restart), both issued through the Machine API.

use talos_api_rs::api::machine::machine_service_client::MachineServiceClient;
use talos_api_rs::api::machine::{
    RebootRequest, ServiceRestartRequest, ServiceStartRequest, ServiceStopRequest, ShutdownRequest,
};

use crate::error::TalosError;
use crate::request::{ensure_metadata, targeted};
use crate::Backend;

impl Backend {
    pub async fn etcd_defragment(&self, node: &str) -> Result<(), TalosError> {
        let channel = self.current_channel().await?;
        let mut client = MachineServiceClient::new(channel);
        let response = client
            .etcd_defragment(targeted(node, ())?)
            .await?
            .into_inner();
        ensure_metadata(
            response
                .messages
                .iter()
                .map(|message| message.metadata.as_ref()),
        )
    }

    pub async fn service_action(
        &self,
        node: &str,
        service: &str,
        action: &str,
    ) -> Result<(), TalosError> {
        let channel = self.current_channel().await?;
        let mut client = MachineServiceClient::new(channel);
        match action {
            "start" => {
                let response = client
                    .service_start(targeted(node, ServiceStartRequest { id: service.into() })?)
                    .await?
                    .into_inner();
                ensure_metadata(
                    response
                        .messages
                        .iter()
                        .map(|message| message.metadata.as_ref()),
                )?;
            }
            "stop" => {
                let response = client
                    .service_stop(targeted(node, ServiceStopRequest { id: service.into() })?)
                    .await?
                    .into_inner();
                ensure_metadata(
                    response
                        .messages
                        .iter()
                        .map(|message| message.metadata.as_ref()),
                )?;
            }
            "restart" => {
                let response = client
                    .service_restart(targeted(
                        node,
                        ServiceRestartRequest { id: service.into() },
                    )?)
                    .await?
                    .into_inner();
                ensure_metadata(
                    response
                        .messages
                        .iter()
                        .map(|message| message.metadata.as_ref()),
                )?;
            }
            _ => {
                return Err(TalosError::Config(format!(
                    "unknown service action {action:?}"
                )))
            }
        }
        Ok(())
    }

    pub async fn power_action(&self, node: &str, action: &str) -> Result<(), TalosError> {
        let channel = self.current_channel().await?;
        let mut client = MachineServiceClient::new(channel);
        match action {
            "reboot" => {
                let response = client
                    .reboot(targeted(node, RebootRequest { mode: 0 })?)
                    .await?
                    .into_inner();
                ensure_metadata(
                    response
                        .messages
                        .iter()
                        .map(|message| message.metadata.as_ref()),
                )?;
            }
            "shutdown" => {
                let response = client
                    .shutdown(targeted(node, ShutdownRequest { force: false })?)
                    .await?
                    .into_inner();
                ensure_metadata(
                    response
                        .messages
                        .iter()
                        .map(|message| message.metadata.as_ref()),
                )?;
            }
            _ => {
                return Err(TalosError::Config(format!(
                    "unknown power action {action:?}"
                )))
            }
        }
        Ok(())
    }
}
