//! People: `/people`, `/profile`, `/networks/{id}/profile`, `/networks/{id}/whois`, `/networks/{id}/members/{memberId}/bot`, `/conversations/{id}/members/{memberId}/recent-messages*`, `/recent-messages-deletions/{jobId}`.
//!
//! Owned by package P1. Also here: `/networks/{id}/ctcp` ("ask their
//! client") and the away flag on `/networks/{id}/self`, which the people
//! views set.

use ircord_contract as c;
use serde::Serialize;

use super::{Api, ApiError, Method, segment};

/// What a people lookup is for; the server offers only the people who can be
/// used that way (a direct message needs a nickname, a group an account).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Purpose {
    Mention,
    Command,
    Mask,
    History,
    Dm,
    Services,
    Party,
}

/// The query of `GET /people`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PeopleQuery {
    pub network_id: String,
    pub purpose: Purpose,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<String>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub query: String,
    /// 1 to 50; the server's default is 20.
    pub limit: u32,
}

impl Api {
    /// `GET /networks/{id}/whois?nick=`: what the IRC server says about a
    /// nick. Quiet to them.
    pub async fn whois(&self, network: &str, nick: &str) -> Result<c::WhoisDetails, ApiError> {
        self.get_query(
            &format!("/api/v1/networks/{}/whois", segment(network)),
            &[("nick", nick)],
        )
        .await
    }

    /// `POST /networks/{id}/ctcp`: ask their client. They may see that you did.
    pub async fn ctcp(
        &self,
        network: &str,
        nick: &str,
        command: c::CtcpRequestCommand,
    ) -> Result<c::CTCPReply, ApiError> {
        let body = c::CtcpRequest {
            command,
            nick: nick.to_string(),
        };
        self.call(
            Method::POST,
            &format!("/api/v1/networks/{}/ctcp", segment(network)),
            &body,
        )
        .await
    }

    /// `GET /people`: people to pick, by nick, account or display name.
    pub async fn people(&self, query: &PeopleQuery) -> Result<c::PeoplePage, ApiError> {
        let mut query = query.clone();
        query.limit = query.limit.clamp(1, 50);
        self.get_query("/api/v1/people", &query).await
    }

    /// `PATCH /networks/{id}/profile`: your profile on one network (your status).
    pub async fn update_network_profile(
        &self,
        network: &str,
        update: &c::ProfileUpdate,
    ) -> Result<c::Member, ApiError> {
        self.call(
            Method::PATCH,
            &format!("/api/v1/networks/{}/profile", segment(network)),
            update,
        )
        .await
    }

    /// `PATCH /networks/{id}/self` with `away`: mark yourself away with
    /// `message`, or back with `None`.
    pub async fn set_away(
        &self,
        network: &str,
        message: Option<&str>,
    ) -> Result<c::Network, ApiError> {
        // `null` clears the flag; the generated type would leave it out.
        let body = serde_json::json!({ "away": message });
        self.call(
            Method::PATCH,
            &format!("/api/v1/networks/{}/self", segment(network)),
            &body,
        )
        .await
    }

    /// `PUT /networks/{id}/members/{memberId}/bot`: as a server owner, mark
    /// someone as a bot or clear the mark. Answers the member as changed.
    pub async fn set_member_bot(
        &self,
        network: &str,
        member: &str,
        bot: bool,
    ) -> Result<c::Member, ApiError> {
        self.call(
            Method::PUT,
            &format!(
                "/api/v1/networks/{}/members/{}/bot",
                segment(network),
                segment(member)
            ),
            &c::SetMemberBotRequest { bot },
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_people_query_is_camel_case_and_leaves_out_what_is_empty() {
        let q = PeopleQuery {
            network_id: "n1".into(),
            purpose: Purpose::Dm,
            conversation_id: None,
            query: String::new(),
            limit: 20,
        };
        assert_eq!(
            serde_json::to_value(&q).unwrap(),
            serde_json::json!({"networkId": "n1", "purpose": "dm", "limit": 20})
        );
    }
}
