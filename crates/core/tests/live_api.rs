//! End-to-end tests against the real db-api.
//!
//! Ignored by default so the normal suite stays hermetic and offline. Run with:
//!
//! ```sh
//! NFD_API_KEY=$(cat ~/.nfd-scanner-dev-key) cargo test -p scanner-core --test live_api -- --ignored
//! ```
//!
//! These assert the contract rather than the corpus, so they should not start
//! failing when rooms are added or edited.

use scanner_core::api::{ApiClient, ApiError, LookupMatch};

fn client() -> ApiClient {
    let key =
        std::env::var("NFD_API_KEY").expect("set NFD_API_KEY to run live tests (see module docs)");
    ApiClient::new(key).expect("client builds")
}

#[tokio::test]
#[ignore = "hits the network"]
async fn resolves_a_room_name_from_a_log_line() {
    // "Start" is the run-start room and appears verbatim in real logs.
    let room = client()
        .resolve_room("Start")
        .await
        .expect("request succeeds")
        .expect("Start is documented");

    assert_eq!(room.case_name.to_lowercase(), "start");
    assert!(!room.slug.is_empty());
}

#[tokio::test]
#[ignore = "hits the network"]
async fn a_squashed_match_still_resolves() {
    // Punctuation and spacing differences must not defeat resolution: the slug
    // for "AltiRoom?" drops the '?', so this only works via the server.
    let lookup = client()
        .lookup("alti room")
        .await
        .expect("request succeeds");

    assert_eq!(lookup.match_kind, LookupMatch::Squashed);
    assert_eq!(lookup.resolved_slug(), Some("altiroom"));
}

#[tokio::test]
#[ignore = "hits the network"]
async fn an_unknown_room_is_none_not_an_error() {
    let result = client()
        .resolve_room("zzzz-definitely-not-a-room")
        .await
        .expect("a miss is a successful request");
    assert!(result.is_none());
}

#[tokio::test]
#[ignore = "hits the network"]
async fn every_room_name_from_the_real_session_resolves() {
    // The pipeline assertion: names as they appear in a Roblox log must resolve
    // without any client-side normalisation.
    use scanner_core::logsrc::{LogEvent, Parser};

    let session = include_str!("fixtures/session.log");
    let names: Vec<String> = Parser::new()
        .parse_lines(session.lines())
        .into_iter()
        .filter_map(|e| match e {
            LogEvent::RoomEntered { name } => Some(name),
            _ => None,
        })
        .collect();

    assert_eq!(names.len(), 41);

    let client = client();
    let mut unresolved = Vec::new();
    for name in &names {
        match client.lookup(name).await {
            Ok(l) if l.resolved_slug().is_some() => {}
            Ok(l) => unresolved.push(format!("{name} ({:?})", l.match_kind)),
            Err(e) => panic!("lookup for {name:?} failed: {e}"),
        }
    }

    assert!(unresolved.is_empty(), "unresolved names: {unresolved:?}");
}

#[tokio::test]
#[ignore = "hits the network"]
async fn a_bad_key_is_reported_as_an_auth_problem() {
    let client = ApiClient::new("not-a-real-key").expect("client builds");
    let err = client
        .lookup("Start")
        .await
        .expect_err("should be rejected");

    assert!(
        err.is_auth_problem(),
        "expected an auth failure the user can act on, got: {err}"
    );
    assert!(!err.is_transient(), "must not be retried");
    assert!(matches!(err, ApiError::Forbidden | ApiError::Unauthorized));
}

#[tokio::test]
#[ignore = "hits the network"]
async fn the_cache_serves_a_repeat_lookup() {
    let client = client();
    client.lookup("Start").await.expect("first request");

    let started = std::time::Instant::now();
    client.lookup("Start").await.expect("second request");

    assert!(
        started.elapsed() < std::time::Duration::from_millis(20),
        "repeat lookup took {:?}; expected a cache hit",
        started.elapsed()
    );
}
