use gpui_kit::test::TestWindowExt;
use gpui_kit::{TestAppContext, VisualTestContext};
use rivet::app::{self, Workspace};
use rivet::core::openapi::Spec;
use rivet::core::postman::PostmanImport;
use rivet::core::{
    Auth, BodyKind, Collection, EnvColor, Environment, Method, RequestSpec, SpecRef, store, vars,
};
use rivet::model::RailMode;
use rivet::theme::Palette;
use serde_json::json;

const SWAGGER: &str = r#"
swagger: '2.0'
info: {title: Legacy API, version: '1.2'}
host: api.example.test
basePath: /v1
schemes: [https]
paths:
  /pets:
    parameters:
      - {name: limit, in: query, type: integer, default: 5}
    post:
      tags: [Pets]
      summary: Create pet
      parameters:
        - {name: payload, in: body, schema: {$ref: '#/definitions/Pet'}}
      responses:
        '201':
          description: created
          schema: {$ref: '#/definitions/Pet'}
definitions:
  Pet:
    type: object
    required: [name]
    properties:
      name: {type: string, example: Fido}
"#;

const POSTMAN: &str = r#"{
  "info": {"name": "Pets", "schema": "https://schema.getpostman.com/json/collection/v2.1.0/collection.json"},
  "variable": [{"key": "baseUrl", "value": "https://pets.example.test"}, {"key": "token", "value": "abc"}],
  "auth": {"type": "bearer", "bearer": [{"key": "token", "value": "{{token}}"}]},
  "item": [{"name": "Pets", "item": [
    {"name": "List", "request": {"method": "GET", "url": {"raw": "{{baseUrl}}/pets?limit=2", "query": [{"key": "limit", "value": "2"}]}, "header": [{"key": "Accept", "value": "application/json"}]}},
    {"name": "Create", "request": {"method": "POST", "url": "{{baseUrl}}/pets", "body": {"mode": "raw", "raw": "{\"name\":\"Fido\"}", "options": {"raw": {"language": "json"}}}}}
  ]}]
}"#;

#[test]
fn swagger_2_imports_operations_server_body_and_response_schema() {
    let spec = Spec::parse(SWAGGER).unwrap();
    assert_eq!(spec.title, "Legacy API");
    assert_eq!(spec.servers, ["https://api.example.test/v1"]);
    let endpoint = spec.find(Method::Post, "/pets").unwrap();
    assert_eq!(endpoint.params[0].value, "5");
    assert_eq!(
        json!("Fido"),
        serde_json::from_str::<serde_json::Value>(endpoint.example_body.as_ref().unwrap()).unwrap()
            ["name"]
    );
    let reference = SpecRef {
        method: Method::Post,
        path: "/pets".into(),
    };
    assert_eq!(
        spec.validate(&reference, 201, &json!({"name":"Fido"})),
        Some(vec![])
    );
    assert_eq!(spec.validate(&reference, 201, &json!({})).unwrap().len(), 1);
}

#[test]
fn openapi_server_variables_become_collection_variables() {
    let spec = Spec::parse("openapi: 3.0.3\ninfo: {title: Regions, version: '1'}\nservers:\n  - url: https://{region}.example.test/api\n    variables:\n      region: {default: eu}\npaths: {}\n").unwrap();
    let vars = spec.collection_variables();
    assert_eq!(vars[0].value, "https://{{region}}.example.test/api");
    assert_eq!(vars[1].value, "eu");
}

#[test]
fn postman_imports_folders_variables_auth_and_bodies() {
    let imported = PostmanImport::parse(POSTMAN).unwrap();
    assert_eq!(imported.collection.name, "Pets");
    assert_eq!(imported.collection.requests.len(), 2);
    assert_eq!(imported.collection.folders, vec!["Pets"]);
    assert_eq!(imported.collection.requests[0].name, "List");
    assert_eq!(
        imported.collection.requests[0].folder.as_deref(),
        Some("Pets")
    );
    assert_eq!(imported.collection.requests[0].url, "{{baseUrl}}/pets");
    assert_eq!(imported.collection.requests[0].params[0].value, "2");
    assert!(matches!(
        imported.collection.requests[0].auth,
        Auth::Bearer { .. }
    ));
    assert_eq!(imported.collection.requests[1].body.kind, BodyKind::Json);
    assert_eq!(imported.collection.vars.len(), 2);
    let old_schema = POSTMAN.replace("v2.1.0", "v2.0.0");
    assert_eq!(
        PostmanImport::parse(&old_schema)
            .unwrap()
            .collection
            .requests
            .len(),
        2
    );
}

#[gpui_kit::test]
fn linked_collection_refresh_keeps_manual_requests(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("api.yaml");
    std::fs::write(&source, "openapi: 3.0.3\ninfo: {title: API, version: '1'}\npaths:\n  /old:\n    get:\n      responses:\n        '200': {description: OK}\n").unwrap();
    let mut col = Collection::new("API");
    col.source = Some(source.clone());
    let mut custom = RequestSpec::new("Custom", Method::Get, "https://example.test/custom");
    custom.collection_id = Some(col.id.clone());
    col.requests.push(custom.clone());
    let mut project = store::seed_project();
    project.spec_path = None;
    project.collections = vec![col];
    cx.update(|cx| {
        gpui_kit::init(cx);
        Palette::dark().apply(cx);
        app::bind_keys(cx);
    });
    let (view, window): (_, &mut VisualTestContext) =
        cx.add_window_view(move |w, cx| Workspace::new(project, None, w, cx));
    std::fs::write(&source, "openapi: 3.0.3\ninfo: {title: API, version: '1'}\npaths:\n  /new:\n    post:\n      responses:\n        '200': {description: OK}\n").unwrap();
    window.update(|w, cx| {
        view.update(cx, |workspace, cx| workspace.reload_spec(w, cx));
    });
    view.read_with(window, |workspace, _| {
        let requests = &workspace.project.collections[0].requests;
        assert_eq!(requests.len(), 2);
        assert!(requests.iter().any(|r| r.id == custom.id));
        assert!(
            requests
                .iter()
                .any(|r| r.spec_ref.as_ref().is_some_and(|s| s.path == "/new"))
        );
    });
}

#[gpui_kit::test]
fn imported_specs_and_postman_appear_as_separate_collections(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let swagger = dir.path().join("legacy.yaml");
    let postman = dir.path().join("pets.postman_collection.json");
    std::fs::write(&swagger, SWAGGER).unwrap();
    std::fs::write(&postman, POSTMAN).unwrap();
    cx.update(|cx| {
        gpui_kit::init(cx);
        Palette::dark().apply(cx);
        app::bind_keys(cx);
    });
    let mut project = store::seed_project();
    project.spec_path = None;
    project.collections.push(Collection::new("Мои запросы"));
    project
        .environments
        .push(Environment::new("Dev", EnvColor::Green));
    let (view, window): (_, &mut VisualTestContext) =
        cx.add_window_view(move |w, cx| Workspace::new(project, None, w, cx));
    window.update(|w, cx| {
        view.update(cx, |workspace, cx| {
            workspace.import_file(swagger, w, cx);
            workspace.import_file(postman, w, cx);
        });
        view.update(cx, |workspace, cx| {
            workspace.open_saved(1, 0, w, cx);
            workspace.copy_curl(cx);
        });
        let curl = cx
            .read_from_clipboard()
            .and_then(|item| item.text())
            .unwrap();
        assert!(curl.contains("https://api.example.test/v1/pets"));
        w.render_frame(cx);
        assert!(w.try_find(("collection", 1usize)).is_some());
        assert!(w.try_find(("collection", 2usize)).is_some());
        assert!(w.try_find(("collection-folder", 1000usize)).is_some());
        assert!(w.try_find(("collection-folder", 2000usize)).is_some());
        view.update(cx, |workspace, cx| {
            workspace.set_rail(RailMode::Collections, w, cx)
        });
        w.render_frame(cx);
        assert!(w.try_find("collection-panel").is_some());
        w.click("collection-var-add", cx);
        w.render_frame(cx);
        view.update(cx, |workspace, cx| {
            workspace.set_rail(RailMode::Settings, w, cx)
        });
        w.render_frame(cx);
        assert!(w.try_find("settings-panel").is_some());
        w.click("settings-ssl", cx);
        w.click("settings-cookies", cx);
    });
    view.read_with(window, |workspace, _| {
        let cols = &workspace.project.collections;
        assert_eq!(
            cols[1].source.as_deref(),
            Some(dir.path().join("legacy.yaml").as_path())
        );
        assert_eq!(
            cols[1].requests[0].collection_id.as_deref(),
            Some(cols[1].id.as_str())
        );
        assert_eq!(
            cols[2].requests[0].collection_id.as_deref(),
            Some(cols[2].id.as_str())
        );
        let mut scoped = workspace.project.environments[0].clone();
        scoped.vars.retain(|v| v.key != "baseUrl");
        scoped.vars.extend(cols[1].vars.clone());
        let resolved = vars::resolve(&cols[1].requests[0], Some(&scoped)).unwrap();
        assert_eq!(resolved.url, "https://api.example.test/v1/pets");
        assert!(!workspace.project.settings.ssl_verify);
        assert!(!workspace.project.settings.cookies_enabled);
        assert_eq!(cols[0].vars.len(), 1);
        let project_file = dir.path().join("project.json");
        store::save(&project_file, &workspace.project).unwrap();
        let reopened = store::load(&project_file).unwrap();
        assert!(!reopened.settings.cookies_enabled);
        assert_eq!(reopened.collections[1].source, cols[1].source);
        assert_eq!(reopened.collections[2].vars, cols[2].vars);
    });
    let updated = SWAGGER.replace(
        "definitions:\n",
        "  /cats:\n    get:\n      responses:\n        '200': {description: ok}\ndefinitions:\n",
    );
    std::fs::write(dir.path().join("legacy.yaml"), updated).unwrap();
    window.update(|w, cx| view.update(cx, |workspace, cx| workspace.reload_spec(w, cx)));
    view.read_with(window, |workspace, _| {
        assert_eq!(workspace.project.collections[1].requests.len(), 2)
    });
}

#[test]
fn postman_imports_request_scripts() {
    let doc = json!({
        "info": {"name": "Scripts", "schema": "https://schema.getpostman.com/json/collection/v2.1.0/collection.json"},
        "item": [
            {
                "name": "Search",
                "event": [
                    {"listen": "prerequest", "script": {"type": "text/javascript", "exec": ["pm.variables.set(\"q\", 1);", "console.log(1);"]}},
                    {"listen": "test", "script": {"type": "text/javascript", "exec": "let response = JSON.parse(responseBody);\npm.environment.set(\"searchId\", response.searchId);"}}
                ],
                "request": {"method": "GET", "url": "{{baseUrl}}/search"}
            },
            {
                "name": "Plain",
                "event": [{"listen": "test", "script": {"exec": [""]}}],
                "request": {"method": "GET", "url": "{{baseUrl}}/plain"}
            }
        ]
    });
    let imported = PostmanImport::parse(&doc.to_string()).unwrap();
    let scripts = &imported.collection.requests[0].scripts;
    assert_eq!(scripts.pre_request, "pm.variables.set(\"q\", 1);\nconsole.log(1);");
    assert_eq!(
        scripts.post_response,
        "let response = JSON.parse(responseBody);\npm.environment.set(\"searchId\", response.searchId);"
    );
    assert!(imported.collection.requests[1].scripts.is_empty());
}
