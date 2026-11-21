//! Groups: `/parties`, `/parties/{id}`, `/parties/{id}/*`, `/parties/candidates`.
//!
//! Owned by package PT1. Also here: answering an invitation
//! (`/invitations/{id}/accept`, `DELETE /invitations/{id}`), for a group or a
//! channel, which the invitation banner does.
//!
//! The server no longer creates groups (`POST /parties` answers 410: "Create
//! an ordinary channel instead"), so there is no call for it; groups made
//! before that are still managed, left and converted to channels here.

use ircord_contract as c;

use super::{Api, ApiError, Method, segment};

fn party_path(id: &str, rest: &str) -> String {
    format!("/api/v1/parties/{}{rest}", segment(id))
}

impl Api {
    /// `GET /parties`: the groups you are in.
    pub async fn parties(&self) -> Result<Vec<c::Party>, ApiError> {
        self.get::<c::ItemsParty>("/api/v1/parties")
            .await
            .map(|page| page.items)
    }

    /// `GET /parties/{id}`.
    pub async fn party(&self, id: &str) -> Result<c::Party, ApiError> {
        self.get(&party_path(id, "")).await
    }

    /// `PATCH /parties/{id}`: change the name and description; `None` keeps
    /// one as it is.
    pub async fn update_party(
        &self,
        id: &str,
        name: Option<String>,
        description: Option<String>,
    ) -> Result<c::Party, ApiError> {
        let body = c::PartyPatchBody { name, description };
        self.call(Method::PATCH, &party_path(id, ""), &body).await
    }

    /// `POST /parties/{id}/invitations`: invite a registered services account.
    pub async fn invite_to_party(
        &self,
        id: &str,
        services_account: &str,
    ) -> Result<c::Invitation, ApiError> {
        let body = c::PartyAccountInputBody {
            services_account: Some(services_account.to_string()),
            ..Default::default()
        };
        self.call(Method::POST, &party_path(id, "/invitations"), &body)
            .await
    }

    /// `DELETE /parties/{id}/members/{memberId}`: take someone out (owner).
    pub async fn remove_party_member(&self, id: &str, member: &str) -> Result<c::Party, ApiError> {
        self.json(self.request(
            Method::DELETE,
            &party_path(id, &format!("/members/{}", segment(member))),
        ))
        .await
    }

    /// `POST /parties/{id}/owner`: hand the group to another member (owner).
    pub async fn transfer_party(&self, id: &str, member: &str) -> Result<c::Party, ApiError> {
        let body = c::PartyAccountInputBody {
            member_id: Some(member.to_string()),
            ..Default::default()
        };
        self.call(Method::POST, &party_path(id, "/owner"), &body)
            .await
    }

    /// `POST /parties/{id}/leave`.
    pub async fn leave_party(&self, id: &str) -> Result<(), ApiError> {
        self.send(self.request(Method::POST, &party_path(id, "/leave")))
            .await
            .map(drop)
    }

    /// `POST /parties/{id}/convert`: make the group an ordinary channel,
    /// keeping its history (owner). May wait on a step on the server.
    pub async fn convert_party(&self, id: &str) -> Result<c::PartyConversionResult, ApiError> {
        self.json(self.request(Method::POST, &party_path(id, "/convert")))
            .await
    }

    /// `POST /invitations/{id}/accept`: join the group or channel; the
    /// answer is its conversation.
    pub async fn accept_invitation(&self, id: &str) -> Result<c::Conversation, ApiError> {
        self.json(self.request(
            Method::POST,
            &format!("/api/v1/invitations/{}/accept", segment(id)),
        ))
        .await
    }

    /// `DELETE /invitations/{id}`: decline. For a channel this also blocks
    /// further invitations to it.
    pub async fn decline_invitation(&self, id: &str) -> Result<(), ApiError> {
        self.delete(&format!("/api/v1/invitations/{}", segment(id)))
            .await
    }
}
