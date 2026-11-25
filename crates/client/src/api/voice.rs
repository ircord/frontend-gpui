//! Voice rooms: `/voice/rooms`, `/voice/rooms/{id}`, `/voice/rooms/{id}/join`
//! and `/voice/sessions/{id}`.
//!
//! The rooms are server-wide: any signed-in account may list and join them,
//! and server owners create, rename and delete them. Joining answers a
//! credential for the server's media connection (see [`crate::voice`]);
//! leaving gives the reservation back.

use ircord_contract as c;

use super::{Api, ApiError, Method, segment};

fn room_path(id: &str, rest: &str) -> String {
    format!("/api/v1/voice/rooms/{}{rest}", segment(id))
}

impl Api {
    /// `GET /voice/rooms`: every room, by name. An older or voiceless server
    /// answers an empty list.
    pub async fn voice_rooms(&self) -> Result<Vec<c::Room>, ApiError> {
        self.get::<Option<Vec<c::Room>>>("/api/v1/voice/rooms")
            .await
            .map(Option::unwrap_or_default)
    }

    /// `POST /voice/rooms`: a new room (server owners).
    pub async fn create_voice_room(&self, settings: &c::RoomSettings) -> Result<c::Room, ApiError> {
        self.call(Method::POST, "/api/v1/voice/rooms", settings)
            .await
    }

    /// `PATCH /voice/rooms/{id}`: rename a room or change its bitrate (server
    /// owners). A new bitrate applies the next time each caller joins.
    pub async fn update_voice_room(
        &self,
        id: &str,
        settings: &c::RoomSettings,
    ) -> Result<c::Room, ApiError> {
        self.call(Method::PATCH, &room_path(id, ""), settings).await
    }

    /// `DELETE /voice/rooms/{id}`: remove a room and end its call (server owners).
    pub async fn delete_voice_room(&self, id: &str) -> Result<(), ApiError> {
        self.delete(&room_path(id, "")).await
    }

    /// `POST /voice/rooms/{id}/join`: reserve a place in the room and get the
    /// media credential. One reservation per account at a time.
    pub async fn join_voice_room(&self, id: &str) -> Result<c::Credential, ApiError> {
        self.json(self.request(Method::POST, &room_path(id, "/join")))
            .await
    }

    /// `DELETE /voice/sessions/{id}`: give the reservation back. The server
    /// also ends it on its own when the media connection goes away.
    pub async fn leave_voice_room(&self, credential: &str) -> Result<(), ApiError> {
        self.delete(&format!("/api/v1/voice/sessions/{}", segment(credential)))
            .await
    }
}

#[cfg(test)]
mod tests {
    use crate::client::runtime;
    use crate::testing::{CannedServer, Reply};
    use ircord_contract as c;
    use serde_json::json;

    #[test]
    fn lists_joins_and_leaves() {
        let rt = runtime().unwrap();
        let server = CannedServer::start(&rt);
        server.route(
            "GET",
            "/api/v1/voice/rooms",
            200,
            json!([{"id": "r1", "name": "Lounge", "bitrateKbps": 64, "participants": ["ann"], "capacity": 8, "available": true}]),
        );
        server.route(
            "POST",
            "/api/v1/voice/rooms/r1/join",
            200,
            json!({"id": "cred1", "url": "wss://chat.example/voice", "token": "jwt"}),
        );
        server.route_reply("DELETE", "/api/v1/voice/sessions/cred1", Reply::empty());
        let api = server.api();
        let rooms = rt.block_on(api.voice_rooms()).unwrap();
        assert_eq!(rooms.len(), 1);
        assert_eq!(rooms[0].name, "Lounge");
        assert_eq!(rooms[0].participants, ["ann"]);
        let credential = rt.block_on(api.join_voice_room("r1")).unwrap();
        assert_eq!(credential.url, "wss://chat.example/voice");
        assert_eq!(credential.token, "jwt");
        // The join is a plain call, never the microphone test.
        let join = &server.requests_to("POST", "/api/v1/voice/rooms/r1/join")[0];
        assert_eq!(join.query_param("echo"), None);
        rt.block_on(api.leave_voice_room("cred1")).unwrap();
        assert_eq!(
            server
                .requests_to("DELETE", "/api/v1/voice/sessions/cred1")
                .len(),
            1
        );
    }

    #[test]
    fn a_server_without_voice_lists_no_rooms() {
        let rt = runtime().unwrap();
        let server = CannedServer::start(&rt);
        server.route("GET", "/api/v1/voice/rooms", 200, json!([]));
        assert!(rt.block_on(server.api().voice_rooms()).unwrap().is_empty());
    }

    #[test]
    fn owners_manage_rooms() {
        let rt = runtime().unwrap();
        let server = CannedServer::start(&rt);
        let room = json!({"id": "r2", "name": "Studio", "bitrateKbps": 96, "participants": [], "capacity": 8, "available": true});
        server.route("POST", "/api/v1/voice/rooms", 201, room.clone());
        server.route("PATCH", "/api/v1/voice/rooms/r2", 200, room);
        server.route_reply("DELETE", "/api/v1/voice/rooms/r2", Reply::empty());
        let api = server.api();
        let settings = c::RoomSettings {
            name: "Studio".into(),
            bitrate_kbps: 96,
        };
        assert_eq!(
            rt.block_on(api.create_voice_room(&settings)).unwrap().id,
            "r2"
        );
        let body = server.requests_to("POST", "/api/v1/voice/rooms")[0].json();
        assert_eq!(body, json!({"name": "Studio", "bitrateKbps": 96}));
        rt.block_on(api.update_voice_room("r2", &settings)).unwrap();
        rt.block_on(api.delete_voice_room("r2")).unwrap();
        assert_eq!(
            server.requests_to("DELETE", "/api/v1/voice/rooms/r2").len(),
            1
        );
    }

    #[test]
    fn a_refused_join_says_why() {
        let rt = runtime().unwrap();
        let server = CannedServer::start(&rt);
        server.route_reply(
            "POST",
            "/api/v1/voice/rooms/r1/join",
            Reply::problem(409, "This voice channel is full."),
        );
        let error = rt.block_on(server.api().join_voice_room("r1")).unwrap_err();
        assert_eq!(error.to_string(), "This voice channel is full.");
    }
}
