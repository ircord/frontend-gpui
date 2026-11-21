//! Activity reports and notification settings: `/activity`,
//! `/notifications/settings`, `/push/*`, and each device's own choices
//! (`/auth/devices/{id}/notifications`).
//!
//! The settings document itself is read and written through
//! `crate::documents::NOTIFICATION_SETTINGS`; package S3 adds the
//! per-device choices here.

use ircord_contract as c;

use super::{Api, ApiError, Method, segment};

/// Why a device's choices were not saved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceSaveError {
    /// The choices changed on another device since they were read (409).
    Changed,
    Failed(String),
}

impl Api {
    pub async fn report_activity(&self, report: &c::Report) -> Result<(), ApiError> {
        self.send(self.request(Method::POST, "/api/v1/activity").json(report))
            .await
            .map(drop)
    }

    /// Save one device's notification choices; the answer is the device as
    /// stored. `preferences.revision` must be the one last read.
    pub async fn save_device_notifications(
        &self,
        device: &str,
        preferences: &c::DevicePreferences,
    ) -> Result<c::Device, ApiError> {
        let path = format!("/api/v1/auth/devices/{}/notifications", segment(device));
        self.call(Method::PUT, &path, preferences).await
    }
}

impl DeviceSaveError {
    pub fn of(error: &ApiError) -> Self {
        match error {
            ApiError::Http { status: 409, .. } => DeviceSaveError::Changed,
            e => DeviceSaveError::Failed(e.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::runtime;
    use crate::testing::{CannedServer, Reply};
    use serde_json::json;

    #[test]
    fn saves_a_device_and_names_a_conflict() {
        let rt = runtime().unwrap();
        let server = CannedServer::start(&rt);
        server.route(
            "PUT",
            "/api/v1/auth/devices/d%2Fhere/notifications",
            200,
            json!({"id": "d/here", "current": true,
                   "notifications": {"initialized": true, "delivery": "open", "content": "all",
                                     "sound": "off", "preview": false, "revision": 4}}),
        );
        server.route_reply(
            "PUT",
            "/api/v1/auth/devices/d-old/notifications",
            Reply::problem(409, "device notification settings changed"),
        );
        let api = server.api();
        let preferences = c::DevicePreferences {
            content: c::DevicePreferencesContent::All,
            delivery: c::DevicePreferencesDelivery::Open,
            sound: c::DevicePreferencesSound::Off,
            preview: false,
            revision: 3,
            name: "Desk".into(),
            migrate: None,
        };
        let saved = rt
            .block_on(api.save_device_notifications("d/here", &preferences))
            .unwrap();
        assert_eq!(saved.notifications.revision, 4);
        let sent = server.requests_to("PUT", "/api/v1/auth/devices/d%2Fhere/notifications");
        assert_eq!(
            sent[0].json(),
            json!({"content": "all", "delivery": "open", "sound": "off", "preview": false,
                   "revision": 3, "name": "Desk"})
        );
        let refused = rt
            .block_on(api.save_device_notifications("d-old", &preferences))
            .unwrap_err();
        assert_eq!(DeviceSaveError::of(&refused), DeviceSaveError::Changed);
    }
}
