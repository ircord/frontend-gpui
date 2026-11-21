//! Devices: `/auth/devices`, `/auth/devices/{id}`, `/auth/devices/{id}/notifications`.
//!
//! Package S0 lists the devices signed in to the account and signs one out;
//! S3 adds the per-device notification settings in this `impl Api` block.

use ircord_contract as c;

use super::{Api, ApiError, Method, segment};

impl Api {
    /// Where this account is signed in, this device first.
    pub async fn devices(&self) -> Result<Vec<c::Device>, ApiError> {
        let list: c::DeviceList = self.get("/api/v1/auth/devices").await?;
        let mut devices = list.devices.unwrap_or_default();
        devices.sort_by_key(|d| !d.current);
        Ok(devices)
    }

    /// Sign a device out: its connections close and its next request is
    /// refused. `Ok(false)` when it was already signed out (404). Signing out
    /// this device also ends this session.
    pub async fn sign_out_device(&self, id: &str) -> Result<bool, ApiError> {
        let path = format!("/api/v1/auth/devices/{}", segment(id));
        match self.send(self.request(Method::DELETE, &path)).await {
            Ok(_) => Ok(true),
            Err(ApiError::Http { status: 404, .. }) => Ok(false),
            Err(e) => Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::client::runtime;
    use crate::testing::{CannedServer, Reply};
    use serde_json::json;

    #[test]
    fn lists_this_device_first_and_signs_one_out() {
        let rt = runtime().unwrap();
        let server = CannedServer::start(&rt);
        server.route(
            "GET",
            "/api/v1/auth/devices",
            200,
            json!({"devices": [
                {"id": "d-phone", "current": false, "name": "", "browser": "Firefox", "os": "Android"},
                {"id": "d-here", "current": true, "name": "", "browser": "IRCord-GPUI", "os": "Linux"}
            ]}),
        );
        server.route_reply("DELETE", "/api/v1/auth/devices/d-phone", Reply::empty());
        server.route_reply(
            "DELETE",
            "/api/v1/auth/devices/d-gone",
            Reply::problem(404, "not found"),
        );
        let api = server.api();
        let devices = rt.block_on(api.devices()).unwrap();
        let ids: Vec<_> = devices.iter().map(|d| d.id.as_str()).collect();
        assert_eq!(ids, ["d-here", "d-phone"]);
        assert!(rt.block_on(api.sign_out_device("d-phone")).unwrap());
        assert!(!rt.block_on(api.sign_out_device("d-gone")).unwrap());
        assert_eq!(
            server
                .requests_to("DELETE", "/api/v1/auth/devices/d-phone")
                .len(),
            1
        );
    }

    #[test]
    fn account_password_deletion_and_release_notes() {
        let rt = runtime().unwrap();
        let server = CannedServer::start(&rt);
        server.route(
            "GET",
            "/api/v1/auth/session",
            200,
            json!({"account": {"id": "a1", "username": "me",
                "createdAt": "2026-01-02T00:00:00Z", "expiresAt": "2027-01-02T00:00:00Z",
                "lastSeenAt": "2026-11-20T00:00:00Z"}}),
        );
        server.route_reply("POST", "/api/v1/auth/password", Reply::empty());
        server.route_reply(
            "DELETE",
            "/api/v1/auth/account",
            Reply::problem(403, "The password is not right."),
        );
        server.route(
            "GET",
            "/api/v1/changelog",
            200,
            json!({"version": "1.2.3", "releases": [
                {"version": "1.2.3", "date": "2026-11-01", "notes": "- Faster."}
            ]}),
        );
        let api = server.api();
        assert_eq!(rt.block_on(api.account()).unwrap().unwrap().username, "me");
        rt.block_on(api.change_password("old", "new password"))
            .unwrap();
        let sent = server.requests_to("POST", "/api/v1/auth/password");
        assert_eq!(
            sent[0].json(),
            json!({"current": "old", "password": "new password"})
        );
        let refused = rt.block_on(api.delete_account("wrong")).unwrap_err();
        assert_eq!(refused.to_string(), "The password is not right.");
        assert!(
            api.cookie().is_some(),
            "a refused deletion keeps the session"
        );
        let notes = rt.block_on(api.changelog()).unwrap();
        assert_eq!(notes.version, "1.2.3");
        assert_eq!(notes.releases.unwrap()[0].notes, "- Faster.");
    }
}
