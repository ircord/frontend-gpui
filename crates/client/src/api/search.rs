//! Search: `/search`.
//!
//! Owned by package FD1. The filters and the query pairs are in
//! [`crate::search_filter`].

use ircord_contract as c;

use super::{Api, ApiError};
use crate::search_filter::Query;

impl Api {
    /// One page of messages matching `query`, newest first. Pass the page's
    /// `nextCursor` back in `query.cursor` for the next, older page.
    pub async fn search(&self, query: &Query) -> Result<c::SearchPage, ApiError> {
        self.get_query("/api/v1/search", &query.pairs()).await
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use crate::search_filter::{Chip, Filters, Has, Query, Source};
    use crate::testing::{CannedServer, Reply};

    #[test]
    fn sends_every_filter_and_reads_the_page() {
        let runtime = crate::client::runtime().unwrap();
        let server = CannedServer::start(&runtime);
        server.route_reply(
            "GET",
            "/api/v1/search",
            Reply::json(
                200,
                serde_json::json!({
                    "items": [{"conversation": {"id": "c1", "networkId": "n1", "kind": "channel",
                                                "name": "general"},
                               "message": {"id": "m9", "conversationId": "c1", "text": "hello there",
                                           "senderId": "m-bob", "createdAt": "2026-11-20T10:00:00Z"}}],
                    "nextCursor": "older",
                    "source": {"requested": "upstream", "actual": "upstream",
                               "coverage": "upstream-bounded", "completeness": "unknown"}
                }),
            ),
        );
        let api = server.api();
        let filters = Filters::default()
            .with(Chip::From("m-bob".into()))
            .with(Chip::From("m-eve".into()))
            .with(Chip::Has(Has::Link))
            .scoped("c1");
        let mut query = Query::new("hello", filters, Source::Upstream);
        query.cursor = Some("abc".into());
        let page = runtime.block_on(api.search(&query)).expect("search");
        assert_eq!(page.items[0].message.id, "m9");
        assert_eq!(page.next_cursor.as_deref(), Some("older"));
        let request = server.wait_for("GET", "/api/v1/search", Duration::from_secs(5));
        let query = request.query.unwrap();
        for part in [
            "q=hello",
            "from=m-bob",
            "from=m-eve",
            "has=link",
            "conversationId=c1",
            "cursor=abc",
            "source=upstream",
        ] {
            assert!(query.split('&').any(|p| p == part), "{part} in {query}");
        }
    }

    #[test]
    fn a_rejected_search_says_why() {
        let runtime = crate::client::runtime().unwrap();
        let server = CannedServer::start(&runtime);
        server.route_reply(
            "GET",
            "/api/v1/search",
            Reply::problem(422, "Enter at least two characters or choose a filter."),
        );
        let error = runtime
            .block_on(server.api().search(&Query::default()))
            .unwrap_err();
        assert!(error.to_string().contains("two characters"), "{error}");
    }
}
