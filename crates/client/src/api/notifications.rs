//! Activity reports and notification settings: `/activity`,
//! `/notifications/settings`, `/push/*`.
//!
//! Package S3 adds the settings document and the test notification.

use ircord_contract as c;

use super::{Api, ApiError, Method};

impl Api {
    pub async fn report_activity(&self, report: &c::Report) -> Result<(), ApiError> {
        self.send(self.request(Method::POST, "/api/v1/activity").json(report))
            .await
            .map(drop)
    }
}
