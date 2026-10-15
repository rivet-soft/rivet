use std::time::{Duration, Instant};

use rivet::core::script;
use rivet::core::{
    Body, BodyKind, KeyValue, LogLevel, Method, RequestSpec, ResponseData, ScriptInput,
    ScriptOutcome, ScriptStage, Timings, VarChange,
};

fn request() -> RequestSpec {
    let mut r = RequestSpec::new("Search", Method::Get, "{{baseUrl}}/search");
    r.headers = vec![
        KeyValue::new("Accept", "application/json").description("keep me"),
        KeyValue::new("X-Old", "1"),
    ];
    r
}

fn response(status: u16, body: &str) -> ResponseData {
    ResponseData {
        status,
        status_text: if status == 200 { "OK".into() } else { "Not Found".into() },
        http_version: "HTTP/1.1".into(),
        remote_addr: None,
        headers: vec![
            ("Content-Type".into(), "application/json".into()),
            ("X-Request-Id".into(), "abc".into()),
        ],
        redirect_history: Vec::new(),
        hops: Vec::new(),
        body: body.as_bytes().to_vec(),
        timings: Timings {
            total: Duration::from_millis(123),
            ..Timings::default()
        },
    }
}

fn post(script_src: &str, body: &str) -> ScriptOutcome {
    post_with(script_src, response(200, body), Vec::new(), Vec::new())
}

fn post_with(
    script_src: &str,
    res: ResponseData,
    env: Vec<KeyValue>,
    collection_vars: Vec<KeyValue>,
) -> ScriptOutcome {
    script::run(
        script_src,
        &ScriptInput {
            stage: ScriptStage::PostResponse,
            request: request(),
            env,
            collection_vars,
            response: Some(res),
        },
    )
}

fn pre(script_src: &str) -> ScriptOutcome {
    pre_with(script_src, Vec::new(), Vec::new())
}

fn pre_with(script_src: &str, env: Vec<KeyValue>, collection_vars: Vec<KeyValue>) -> ScriptOutcome {
    script::run(
        script_src,
        &ScriptInput {
            stage: ScriptStage::PreRequest,
            request: request(),
            env,
            collection_vars,
            response: None,
        },
    )
}

fn set(key: &str, value: &str) -> VarChange {
    VarChange {
        key: key.into(),
        value: Some(value.into()),
    }
}

fn ok(outcome: &ScriptOutcome) {
    assert!(outcome.error.is_none(), "unexpected error: {:?}", outcome.error);
}

#[test]
fn user_example_saves_search_id() {
    let outcome = post(
        "let response = JSON.parse(responseBody); pm.environment.set(\"searchId\", response.searchId);",
        r#"{"searchId":"s-42","items":[]}"#,
    );
    ok(&outcome);
    assert_eq!(outcome.env_changes, vec![set("searchId", "s-42")]);
    assert!(outcome.collection_changes.is_empty());
    assert!(outcome.request.is_none(), "post-response returns no request");
}

#[test]
fn response_api_and_value_coercion() {
    let outcome = post(
        r#"
        const data = pm.response.json();
        pm.environment.set("id", data.id);
        pm.environment.set("obj", data.nested);
        pm.environment.set("flag", true);
        pm.environment.set("code", pm.response.code + ":" + pm.response.status);
        pm.environment.set("rid", pm.response.headers.get("x-request-id"));
        pm.environment.set("time", pm.response.responseTime);
        pm.environment.set("legacy", responseCode.code);
        pm.environment.set("id", data.id + 1);
        pm.environment.set("len", pm.response.text().length);
        "#,
        r#"{"id":7,"nested":{"a":[1,2]}}"#,
    );
    ok(&outcome);
    assert_eq!(
        outcome.env_changes,
        vec![
            set("id", "8"),
            set("obj", r#"{"a":[1,2]}"#),
            set("flag", "true"),
            set("code", "200:OK"),
            set("rid", "abc"),
            set("time", "123"),
            set("legacy", "200"),
            set("len", "29"),
        ]
    );
}

#[test]
fn variables_priority_local_collection_environment() {
    let env = vec![
        KeyValue::new("a", "env"),
        KeyValue::new("b", "env"),
        KeyValue::new("c", "env"),
        KeyValue::new("off", "x").enabled(false),
    ];
    let coll = vec![KeyValue::new("a", "coll"), KeyValue::new("b", "coll")];
    let outcome = pre_with(
        r#"
        pm.variables.set("a", "local");
        console.log(pm.variables.get("a"), pm.variables.get("b"), pm.variables.get("c"),
            String(pm.variables.get("missing")), pm.variables.has("off"), pm.variables.has("c"));
        console.log(pm.variables.replaceIn("{{a}}-{{b}}-{{c}}-{{nope}}"));
        console.log(pm.environment.get("a"), pm.collectionVariables.get("a"));
        console.log(pm.environment.replaceIn("{{a}}"));
        "#,
        env,
        coll,
    );
    ok(&outcome);
    let texts: Vec<_> = outcome.logs.iter().map(|l| l.text.as_str()).collect();
    assert_eq!(
        texts,
        vec![
            "local coll env undefined false true",
            "local-coll-env-{{nope}}",
            "env coll",
            "env",
        ]
    );
    // pm.variables.set is local only.
    assert!(outcome.env_changes.is_empty());
    assert!(outcome.collection_changes.is_empty());
}

#[test]
fn unset_and_collection_variables() {
    let env = vec![KeyValue::new("token", "t"), KeyValue::new("keep", "k")];
    let outcome = pre_with(
        r#"
        pm.environment.set("x", "1");
        pm.environment.unset("token");
        pm.environment.set("x", "2");
        pm.collectionVariables.set("page", 3);
        pm.collectionVariables.unset("gone");
        console.log(pm.environment.has("token"), pm.environment.has("keep"), JSON.stringify(pm.environment.toObject()));
        postman.setEnvironmentVariable("legacy", "L");
        postman.clearEnvironmentVariable("keep");
        postman.setGlobalVariable("g", "G");
        console.log(postman.getEnvironmentVariable("legacy"));
        "#,
        env,
        Vec::new(),
    );
    ok(&outcome);
    assert_eq!(
        outcome.env_changes,
        vec![
            set("x", "2"),
            VarChange { key: "token".into(), value: None },
            set("legacy", "L"),
            VarChange { key: "keep".into(), value: None },
            set("g", "G"),
        ]
    );
    assert_eq!(
        outcome.collection_changes,
        vec![set("page", "3"), VarChange { key: "gone".into(), value: None }]
    );
    assert_eq!(outcome.logs[0].text, r#"false true {"keep":"k","x":"2"}"#);
    assert_eq!(outcome.logs[1].text, "L");
}

#[test]
fn pre_request_modifies_request() {
    let outcome = pre(
        r#"
        console.log(pm.request.url.toString(), pm.request.method, pm.info.requestName, pm.info.eventName);
        pm.request.url = pm.request.url.toString() + "?q=1";
        pm.request.method = "post";
        pm.request.headers.add({ key: "X-New", value: "n" });
        pm.request.headers.upsert({ key: "accept", value: "text/plain" });
        pm.request.headers.remove("X-Old");
        pm.request.body.raw = JSON.stringify({ a: 1 });
        console.log(pm.request.headers.get("x-new"), pm.request.headers.has("X-Old"), `${pm.request.url}`);
        "#,
    );
    ok(&outcome);
    assert_eq!(outcome.logs[0].text, "{{baseUrl}}/search GET Search prerequest");
    assert_eq!(outcome.logs[1].text, "n false {{baseUrl}}/search?q=1");
    let r = outcome.request.expect("pre-request returns the request");
    assert_eq!(r.url, "{{baseUrl}}/search?q=1");
    assert_eq!(r.method, Method::Post);
    assert_eq!(r.headers.len(), 2);
    assert_eq!(r.headers[0].key, "Accept");
    assert_eq!(r.headers[0].value, "text/plain");
    assert_eq!(r.headers[0].description, "keep me");
    assert_eq!(r.headers[1].key, "X-New");
    assert_eq!(r.headers[1].value, "n");
    assert!(r.headers[1].enabled);
    assert_eq!(r.body.kind, BodyKind::Text);
    assert_eq!(r.body.text, r#"{"a":1}"#);
}

#[test]
fn pre_request_without_changes_keeps_request() {
    let original = request();
    let outcome = script::run(
        "// nothing",
        &ScriptInput {
            stage: ScriptStage::PreRequest,
            request: original.clone(),
            env: Vec::new(),
            collection_vars: Vec::new(),
            response: None,
        },
    );
    ok(&outcome);
    assert_eq!(outcome.request, Some(original));

    let mut json_req = request();
    json_req.body = Body::json("{}");
    let outcome = script::run(
        "pm.request.body.raw = '{\"b\":2}';",
        &ScriptInput {
            stage: ScriptStage::PreRequest,
            request: json_req,
            env: Vec::new(),
            collection_vars: Vec::new(),
            response: None,
        },
    );
    ok(&outcome);
    let body = outcome.request.unwrap().body;
    assert_eq!(body.kind, BodyKind::Json);
    assert_eq!(body.text, r#"{"b":2}"#);
}

#[test]
fn pm_test_pass_and_fail() {
    let outcome = post(
        r#"
        pm.test("Status code is 200", function () { pm.response.to.have.status(200); });
        pm.test("Has id", () => { pm.expect(pm.response.json().id).to.equal(2); });
        pm.test("Header", () => { pm.response.to.have.header("Content-Type"); pm.response.to.be.ok; });
        pm.test("Throws type error", () => { undefined.x; });
        pm.test("Time", () => { pm.expect(pm.response.responseTime).to.be.below(500); });
        console.log("after tests");
        "#,
        r#"{"id":1}"#,
    );
    ok(&outcome);
    let summary: Vec<_> = outcome
        .tests
        .iter()
        .map(|t| (t.name.as_str(), t.passed))
        .collect();
    assert_eq!(
        summary,
        vec![
            ("Status code is 200", true),
            ("Has id", false),
            ("Header", true),
            ("Throws type error", false),
            ("Time", true),
        ]
    );
    assert_eq!(
        outcome.tests[1].error.as_deref(),
        Some("AssertionError: expected 1 to equal 2")
    );
    assert!(outcome.tests[3].error.as_deref().unwrap().starts_with("TypeError"));
    assert_eq!(outcome.logs.last().unwrap().text, "after tests");

    let failed = post_with(
        "pm.test('status', () => pm.response.to.have.status(200)); pm.test('ok', () => pm.response.to.be.ok);",
        response(404, "nope"),
        Vec::new(),
        Vec::new(),
    );
    assert!(failed.tests.iter().all(|t| !t.passed));
    assert!(failed.tests[0].error.as_deref().unwrap().contains("404"));
}

#[test]
fn legacy_tests_object() {
    let outcome = post(
        r#"tests["x"] = true; tests["Body has id"] = responseBody.has ? false : responseBody.indexOf("id") > -1; tests["bad"] = responseCode.code === 500;"#,
        r#"{"id":1}"#,
    );
    ok(&outcome);
    let summary: Vec<_> = outcome
        .tests
        .iter()
        .map(|t| (t.name.as_str(), t.passed))
        .collect();
    assert_eq!(summary, vec![("x", true), ("Body has id", true), ("bad", false)]);
    assert!(outcome.tests[2].error.is_some());
}

#[test]
fn expect_chains() {
    let outcome = post(
        r#"
        const cases = {
          equal: () => pm.expect(1).to.equal(1),
          notEqual: () => pm.expect(1).to.not.equal(2),
          deepEql: () => pm.expect({ a: [1, { b: 2 }] }).to.eql({ a: [1, { b: 2 }] }),
          notDeepEql: () => pm.expect({ a: 1 }).to.not.eql({ a: 2 }),
          deepEqual: () => pm.expect([1, 2]).to.deep.equal([1, 2]),
          flags: () => { pm.expect(true).to.be.true; pm.expect(false).to.be.false; pm.expect(1).to.be.ok;
                         pm.expect(null).to.be.null; pm.expect(undefined).to.be.undefined;
                         pm.expect([]).to.be.empty; pm.expect("").to.be.empty; pm.expect({}).to.be.empty;
                         pm.expect([1]).to.not.be.empty; pm.expect(0).to.not.be.ok; },
          include: () => { pm.expect("hello").to.include("ell"); pm.expect([1, 2]).to.contain(2);
                           pm.expect({ a: 1, b: 2 }).to.include("a"); pm.expect({ a: 1, b: 2 }).to.include({ b: 2 });
                           pm.expect([1]).to.not.include(3); },
          property: () => { pm.expect({ a: { b: 1 } }).to.have.property("a").that.has.property("b", 1);
                            pm.expect({}).to.not.have.property("x"); },
          length: () => { pm.expect([1, 2, 3]).to.have.lengthOf(3); pm.expect("ab").to.have.length(2); },
          compare: () => { pm.expect(5).to.be.above(4).and.below(6); pm.expect(5).to.be.at.least(5).and.at.most(5); },
          types: () => { pm.expect("s").to.be.a("string"); pm.expect([]).to.be.an("array"); pm.expect({}).to.be.an("object");
                         pm.expect(null).to.be.a("null"); pm.expect(1).to.not.be.a("string"); },
          match: () => pm.expect("abc-123").to.match(/^abc-\d+$/),
          oneOf: () => pm.expect(201).to.be.oneOf([200, 201]),
          failNot: () => pm.expect(1).to.not.equal(1),
          failDeep: () => pm.expect({ a: 1 }).to.eql({ a: "1" }),
          failInclude: () => pm.expect([1, 2]).to.include(5),
          failType: () => pm.expect(1).to.be.a("string"),
          failAbove: () => pm.expect(1).to.be.above(2),
        };
        Object.keys(cases).forEach(name => pm.test(name, cases[name]));
        "#,
        "{}",
    );
    ok(&outcome);
    for t in &outcome.tests {
        let should_pass = !t.name.starts_with("fail");
        assert_eq!(t.passed, should_pass, "{}: {:?}", t.name, t.error);
    }
    let error = |name: &str| {
        outcome
            .tests
            .iter()
            .find(|t| t.name == name)
            .and_then(|t| t.error.clone())
            .unwrap()
    };
    assert_eq!(error("failNot"), "AssertionError: expected 1 to not equal 1");
    assert_eq!(
        error("failDeep"),
        r#"AssertionError: expected {"a":1} to deeply equal {"a":"1"}"#
    );
    assert_eq!(error("failType"), "AssertionError: expected 1 to be a string");
    assert_eq!(outcome.tests.len(), 18);
}

#[test]
fn syntax_error_reports_line() {
    let outcome = pre("let a = 1;\nlet b = 2;\nlet c = ;\n");
    let error = outcome.error.expect("syntax error");
    assert_eq!(error.line, Some(3), "{error:?}");
    assert!(error.message.starts_with("SyntaxError"), "{error:?}");

    // Unclosed brace: the parser fails at the end of the script, not in the wrapper.
    let outcome = pre("if (true) {\n  console.log(1);\n");
    let error = outcome.error.expect("syntax error");
    assert_eq!(error.line, Some(2), "{error:?}");
}

#[test]
fn runtime_error_reports_line() {
    let outcome = post(
        "pm.environment.set('before', 'yes');\nconst a = 1;\nnotDefined.call();\nconsole.log('never');",
        "{}",
    );
    let error = outcome.error.expect("runtime error");
    assert_eq!(error.line, Some(3), "{error:?}");
    assert!(error.message.contains("notDefined"), "{error:?}");
    // Changes before the error are kept, later statements do not run.
    assert_eq!(outcome.env_changes, vec![set("before", "yes")]);
    assert!(outcome.logs.is_empty());

    let outcome = post("\n\nthrow new Error('boom');", "{}");
    let error = outcome.error.expect("thrown error");
    assert_eq!(error.message, "Error: boom");
    assert_eq!(error.line, Some(3), "{error:?}");

    // Error thrown inside the prelude (invalid JSON) points at the user's line.
    let outcome = post("const x = 1;\nconst d = pm.response.json();", "not json");
    let error = outcome.error.expect("json error");
    assert_eq!(error.line, Some(2), "{error:?}");
    assert!(error.message.starts_with("SyntaxError"), "{error:?}");

    // Assertion outside pm.test fails the script with its line.
    let outcome = post("\npm.response.to.have.status(201);", "{}");
    let error = outcome.error.expect("assertion error");
    assert_eq!(error.line, Some(2), "{error:?}");
    assert!(error.message.starts_with("AssertionError"), "{error:?}");

    let outcome = post("throw 'plain';", "{}");
    assert!(outcome.error.unwrap().message.contains("plain"));

    // Bare reference at top level, and inside a function.
    let outcome = post("const a = 1;\n// other line\nlet b = a + missingVar;", "{}");
    assert_eq!(outcome.error.unwrap().line, Some(3));
    let outcome = post("console.log(1);\nfunction f() {\n  return q.w;\n}\nf();", "{}");
    assert_eq!(outcome.error.unwrap().line, Some(3));
    let outcome = post("console.log(1);\nif (true) {\n  null.x;\n}", "{}");
    assert_eq!(outcome.error.unwrap().line, Some(3));
}

#[test]
fn infinite_loop_stops_quickly() {
    let started = Instant::now();
    let outcome = pre("console.log('start');\nwhile (true) {}");
    let elapsed = started.elapsed();
    eprintln!("infinite loop stopped after {elapsed:?}");
    let error = outcome.error.expect("loop limit error");
    assert!(error.message.contains("loop iterations"), "{error:?}");
    assert_eq!(outcome.logs.len(), 1, "output before the loop is kept");
    assert!(elapsed < Duration::from_secs(20), "took {elapsed:?}");

    let outcome = pre("function f() { return f(); }\nf();");
    let error = outcome.error.expect("recursion error");
    assert!(error.message.contains("depth"), "{error:?}");
}

#[test]
fn console_levels() {
    let outcome = pre(
        "console.log('a', 1, {x: [1]}, null, undefined);\nconsole.info('i');\nconsole.warn('w');\nconsole.error(new Error('e'));",
    );
    ok(&outcome);
    let logs: Vec<_> = outcome
        .logs
        .iter()
        .map(|l| (l.level, l.text.as_str()))
        .collect();
    assert_eq!(
        logs,
        vec![
            (LogLevel::Log, r#"a 1 {"x":[1]} null undefined"#),
            (LogLevel::Info, "i"),
            (LogLevel::Warn, "w"),
            (LogLevel::Error, "Error: e"),
        ]
    );
    assert!(outcome.duration > Duration::ZERO);
}

#[test]
fn performance_parse_one_megabyte_json() {
    let mut body = String::from("{\"searchId\":\"perf\",\"items\":[");
    let mut i = 0;
    while body.len() < 1_000_000 {
        if i > 0 {
            body.push(',');
        }
        body.push_str(&format!(
            "{{\"id\":{i},\"name\":\"item number {i}\",\"price\":{}.5,\"tags\":[\"a\",\"b\"],\"active\":true}}",
            i % 100
        ));
        i += 1;
    }
    body.push_str("]}");
    let started = Instant::now();
    let outcome = post(
        "const r = JSON.parse(responseBody);\npm.environment.set('searchId', r.searchId);\npm.environment.set('count', r.items.length);",
        &body,
    );
    let elapsed = started.elapsed();
    eprintln!(
        "PERF: JSON.parse of {} bytes ({i} items): {elapsed:?} total, script {:?}",
        body.len(),
        outcome.duration
    );
    ok(&outcome);
    assert_eq!(
        outcome.env_changes,
        vec![set("searchId", "perf"), set("count", &i.to_string())]
    );
    assert!(elapsed < Duration::from_secs(5), "took {elapsed:?}");
}

/// Ordinary scripts and the runtime limits must work on a normal 2 MB thread
/// (the default for spawned threads), without relying on a huge stack.
#[test]
fn runs_on_a_two_megabyte_thread() {
    let handle = std::thread::Builder::new()
        .stack_size(2 * 1024 * 1024)
        .spawn(|| {
            let outcome = post(
                "let response = JSON.parse(responseBody); pm.environment.set(\"searchId\", response.searchId);",
                r#"{"searchId":"s-42"}"#,
            );
            ok(&outcome);
            assert_eq!(outcome.env_changes, vec![set("searchId", "s-42")]);

            let outcome = pre("function f(n) { return f(n + 1); }\nf(0);");
            assert!(outcome.error.is_some());

            let outcome = pre("pm.test('t', () => pm.expect({a: {b: [1, {c: 2}]}}).to.eql({a: {b: [1, {c: 2}]}}));");
            ok(&outcome);
            assert!(outcome.tests[0].passed);

            // Deeply nested JSON from a server: JSON.parse stops at depth 128.
            let depth = 2_000;
            let body = format!("{}{}", "[".repeat(depth), "]".repeat(depth));
            let outcome = post("const x = 1;\nconst d = pm.response.json();", &body);
            let error = outcome.error.expect("nesting error");
            assert!(error.message.contains("recursion limit"), "{error:?}");
            assert_eq!(error.line, Some(2));
        })
        .unwrap();
    handle.join().expect("script run on a 2 MB thread");
}

#[test]
fn absurd_nesting_is_rejected_before_parsing() {
    let src = format!("const ok = '((((';\n// [[[[\nconst x = {}1{};", "(".repeat(5_000), ")".repeat(5_000));
    let error = pre(&src).error.expect("nesting error");
    assert!(error.message.contains("nested"), "{error:?}");
    assert_eq!(error.line, Some(3));
    // Brackets in strings and comments do not count.
    let src = format!("const s = '{}';\n/* {} */\nconsole.log(s.length);", "(".repeat(1_000), "[".repeat(1_000));
    ok(&pre(&src));
}
