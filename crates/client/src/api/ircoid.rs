//! Ircoid's high score table: `/ircoid/scores`. A server without the table
//! answers 404; the game plays without it.

use ircord_contract::{IrcoidResult, IrcoidScoreInputBody, IrcoidTable};

use super::{Api, ApiError, Method};

impl Api {
    /// The instance-wide top 10, best first.
    pub async fn ircoid_scores(&self) -> Result<IrcoidTable, ApiError> {
        self.get("/api/v1/ircoid/scores").await
    }

    /// Enters a finished game's score under `name`.
    pub async fn submit_ircoid_score(
        &self,
        name: &str,
        score: u32,
    ) -> Result<IrcoidResult, ApiError> {
        let body = IrcoidScoreInputBody {
            name: name.to_string(),
            score: i64::from(score),
        };
        self.call(Method::POST, "/api/v1/ircoid/scores", &body)
            .await
    }
}

#[cfg(test)]
mod tests {
    use crate::api::ApiError;
    use crate::client::runtime;
    use crate::testing::{CannedServer, Reply};
    use serde_json::json;

    #[test]
    fn reads_the_table() {
        let rt = runtime().unwrap();
        let server = CannedServer::start(&rt);
        server.route(
            "GET",
            "/api/v1/ircoid/scores",
            200,
            json!({"items": [
                {"rank": 1, "name": "Peter Petermann", "score": 225357},
                {"rank": 2, "name": "Michael Karbowiak", "score": 2102}
            ]}),
        );
        let table = rt.block_on(server.api().ircoid_scores()).unwrap();
        let items = table.items.unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].name, "Peter Petermann");
        assert_eq!(items[1].score, 2102);
    }

    #[test]
    fn a_server_without_the_table_answers_404() {
        let rt = runtime().unwrap();
        let server = CannedServer::start(&rt);
        let err = rt.block_on(server.api().ircoid_scores()).unwrap_err();
        assert!(matches!(err, ApiError::Http { status: 404, .. }), "{err:?}");
    }

    #[test]
    fn submits_a_name_and_a_score() {
        let rt = runtime().unwrap();
        let server = CannedServer::start(&rt);
        server.route(
            "POST",
            "/api/v1/ircoid/scores",
            200,
            json!({"placed": true, "rank": 2, "items": [
                {"rank": 1, "name": "Peter Petermann", "score": 225357},
                {"rank": 2, "name": "me", "score": 4560}
            ]}),
        );
        let result = rt
            .block_on(server.api().submit_ircoid_score("me", 4560))
            .unwrap();
        assert!(result.placed);
        assert_eq!(result.rank, 2);
        let sent = server.requests_to("POST", "/api/v1/ircoid/scores");
        assert_eq!(sent[0].json(), json!({"name": "me", "score": 4560}));
    }

    #[test]
    fn a_refused_submission_carries_the_reason() {
        let rt = runtime().unwrap();
        let server = CannedServer::start(&rt);
        server.route_reply(
            "POST",
            "/api/v1/ircoid/scores",
            Reply::problem(422, "Enter a name of 1 to 24 characters."),
        );
        let err = rt
            .block_on(server.api().submit_ircoid_score("", 10))
            .unwrap_err();
        assert_eq!(err.to_string(), "Enter a name of 1 to 24 characters.");
    }
}
