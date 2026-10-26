//! Live mock server in Docker (WireMock). Needs a running Docker daemon, so
//! it is ignored by default: `cargo test --test mock_docker -- --ignored`.

use rivet::core::http::{CancelToken, execute};
use rivet::core::mock::{self, DockerStatus, MockSettings};
use rivet::core::{Collection, Example, KeyValue, Method, RequestSpec, ResolvedRequest, Settings};

fn example(name: &str, status: u16, body: &str) -> Example {
    Example {
        id: name.into(),
        name: name.into(),
        status,
        status_text: String::new(),
        headers: vec![KeyValue::new("Content-Type", "application/json")],
        body: body.into(),
        url: None,
        saved_at: 0,
    }
}

fn get(url: &str, header: Option<(&str, &str)>) -> (u16, String) {
    let req = ResolvedRequest {
        method: Method::Get,
        url: url.into(),
        headers: header.map(|(k, v)| vec![(k.to_string(), v.to_string())]).unwrap_or_default(),
        body: None,
        missing_vars: vec![],
    };
    let r = execute(&req, &Settings::default(), &CancelToken::new()).unwrap();
    (r.status, String::from_utf8_lossy(&r.body).into_owned())
}

#[test]
#[ignore]
fn wiremock_in_docker_serves_examples() {
    let DockerStatus::Ready { docker, version } = mock::docker_status() else {
        panic!("Docker is not available: {:?}", mock::docker_status());
    };
    eprintln!("docker {version} at {}", docker.display());
    let mut c = Collection::new("Live");
    let mut users = RequestSpec::new("Users", Method::Get, "{{baseUrl}}/users/:id");
    users.examples = vec![example("found", 200, r#"{"id":1}"#), example("missing", 404, r#"{"error":"no"}"#)];
    c.requests.push(users);
    let dir = std::env::temp_dir().join(format!("rivet-live-mock-{}", rivet::core::new_id()));
    let settings = MockSettings::default();
    let port = mock::free_port(settings.port, &[]).unwrap();
    let run = mock::start(&docker, &settings, &c, &dir, port).expect("mock starts");
    let result = std::panic::catch_unwind(|| {
        assert_eq!(get(&format!("{}/users/7", run.url), None), (200, r#"{"id":1}"#.into()));
        assert_eq!(get(&format!("{}/users/7", run.url), Some(("x-mock-response-name", "missing"))).0, 404);
        assert_eq!(get(&format!("{}/users/7", run.url), Some(("x-mock-response-code", "404"))).0, 404);
        assert_eq!(get(&format!("{}/nothing", run.url), None).0, 404, "unknown path");
    });
    // Reload with a changed example, without restarting.
    c.requests[0].examples[0].body = r#"{"id":2}"#.into();
    let reloaded = mock::reload(&run, &c).expect("reload");
    let after = get(&format!("{}/users/7", reloaded.url), None);
    mock::stop(&docker, &run).expect("stop");
    std::fs::remove_dir_all(&dir).ok();
    result.unwrap();
    assert_eq!(after, (200, r#"{"id":2}"#.into()));
}
