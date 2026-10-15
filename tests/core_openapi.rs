use std::path::Path;

use rivet::core::openapi::Spec;
use rivet::core::schema::{example_from_schema, line_of_path};
use rivet::core::{BodyKind, SpecRef};
use rivet::model::Method;
use serde_json::{Value, json};

fn shop() -> Spec {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/shop-api.yaml");
    Spec::load(&path).expect("shop-api.yaml parses")
}

fn r(method: Method, path: &str) -> SpecRef {
    SpecRef {
        method,
        path: path.into(),
    }
}

#[test]
fn generates_body_in_the_media_type_declared_by_spec() {
    let spec = Spec::parse(
        r#"
openapi: 3.0.3
info: {title: Uploads, version: '1'}
paths:
  /upload:
    post:
      requestBody:
        content:
          multipart/form-data:
            schema:
              type: object
              properties:
                file: {type: string, format: binary}
                caption: {type: string, example: hello}
      responses: {'200': {description: OK}}
  /form:
    post:
      requestBody:
        content:
          application/x-www-form-urlencoded:
            example: {name: Alice, count: 2}
      responses: {'200': {description: OK}}
  /text:
    post:
      requestBody:
        content:
          text/plain:
            example: hello world
      responses: {'200': {description: OK}}
  /binary:
    post:
      requestBody:
        content:
          application/octet-stream:
            schema: {type: string, format: binary}
      responses: {'200': {description: OK}}
"#,
    )
    .unwrap();

    let upload = spec
        .generate_request_body(&r(Method::Post, "/upload"))
        .unwrap();
    assert_eq!(upload.body.kind, BodyKind::Multipart);
    assert_eq!(upload.content_type, None);
    assert!(
        upload
            .body
            .form
            .iter()
            .any(|field| field.key == "file" && field.file)
    );
    assert!(
        upload
            .body
            .form
            .iter()
            .any(|field| field.key == "caption" && field.value == "hello")
    );

    let form = spec
        .generate_request_body(&r(Method::Post, "/form"))
        .unwrap();
    assert_eq!(form.body.kind, BodyKind::Form);
    assert!(
        form.body
            .form
            .iter()
            .any(|field| field.key == "name" && field.value == "Alice")
    );
    assert!(
        form.body
            .form
            .iter()
            .any(|field| field.key == "count" && field.value == "2")
    );

    let text = spec
        .generate_request_body(&r(Method::Post, "/text"))
        .unwrap();
    assert_eq!(text.body.kind, BodyKind::Text);
    assert_eq!(text.body.text, "hello world");
    assert_eq!(text.content_type.as_deref(), Some("text/plain"));

    let binary = spec
        .generate_request_body(&r(Method::Post, "/binary"))
        .unwrap();
    assert_eq!(binary.body.kind, BodyKind::Binary);
    assert_eq!(
        binary.content_type.as_deref(),
        Some("application/octet-stream")
    );
}

#[test]
fn swagger_form_data_generation_uses_form_format() {
    let spec = Spec::parse(
        r#"
swagger: '2.0'
info: {title: Legacy, version: '1'}
paths:
  /upload:
    post:
      parameters:
        - {in: formData, name: file, type: file}
        - {in: formData, name: title, type: string, default: report}
      responses: {'200': {description: OK}}
"#,
    )
    .unwrap();
    let generated = spec
        .generate_request_body(&r(Method::Post, "/upload"))
        .unwrap();
    assert_eq!(generated.body.kind, BodyKind::Multipart);
    assert!(
        generated
            .body
            .form
            .iter()
            .any(|field| field.key == "file" && field.file)
    );
    assert!(
        generated
            .body
            .form
            .iter()
            .any(|field| field.key == "title" && field.value == "report")
    );
}

#[test]
fn metadata_and_groups() {
    let spec = shop();
    assert_eq!(spec.title, "Shop API");
    assert_eq!(spec.version, "2.3.0");
    assert_eq!(
        spec.servers,
        vec!["https://dev.shop.local/api", "https://api.shop.com/v2"]
    );

    let layout: Vec<(String, Vec<(Method, String, String)>)> = spec
        .groups
        .iter()
        .map(|g| {
            (
                g.name.clone(),
                g.endpoints
                    .iter()
                    .map(|e| (e.method, e.name.clone(), e.path.clone()))
                    .collect(),
            )
        })
        .collect();
    // Same layout as model::demo_groups().
    let expected: Vec<(String, Vec<(Method, String, String)>)> = rivet::model::demo_groups()
        .into_iter()
        .map(|g| {
            (
                g.name.to_string(),
                g.endpoints
                    .iter()
                    .map(|e| (e.method, e.name.to_string(), e.path.to_string()))
                    .collect(),
            )
        })
        .collect();
    assert_eq!(layout, expected);
}

#[test]
fn params_and_auth() {
    let spec = shop();
    let list = spec.find(Method::Get, "/orders").unwrap();
    let keys: Vec<&str> = list.params.iter().map(|p| p.key.as_str()).collect();
    assert_eq!(keys, ["status", "limit", "cursor"]);
    assert!(
        list.params.iter().all(|p| !p.enabled),
        "optional params start disabled"
    );
    assert_eq!(list.params[0].value, "paid");
    assert_eq!(list.params[0].description, "Фильтр по статусу заказа");
    assert_eq!(list.params[1].value, "20");
    assert_eq!(list.params[2].value, "");
    assert!(list.requires_auth);
    assert!(list.example_body.is_none());

    // Path params are not query params.
    assert!(
        spec.find(Method::Get, "/orders/{id}")
            .unwrap()
            .params
            .is_empty()
    );
    // Concrete paths resolve to templates.
    assert_eq!(
        spec.find(Method::Delete, "/orders/ord_1?x=1").unwrap().name,
        "Delete order"
    );

    assert!(
        !spec
            .find(Method::Post, "/auth/login")
            .unwrap()
            .requires_auth
    );
    assert!(
        spec.find(Method::Post, "/auth/refresh")
            .unwrap()
            .requires_auth
    );
    assert!(spec.find(Method::Put, "/orders").is_none());

    let req = list.to_request();
    assert_eq!(req.url, "{{baseUrl}}/orders");
    assert_eq!(req.spec_ref, Some(r(Method::Get, "/orders")));
}

#[test]
fn create_order_example_body() {
    let spec = shop();
    let text = spec
        .example_body(&r(Method::Post, "/orders"))
        .expect("example body");
    assert!(text.contains("\n  \""), "2-space pretty print: {text}");
    let v: Value = serde_json::from_str(&text).unwrap();
    assert!(v["customerId"].is_string());
    assert!(v["items"][0]["quantity"].as_i64().unwrap() >= 1);
    assert!(v["items"][0]["productId"].is_string());
    assert!(v["shippingAddress"]["city"].is_string());
    // Order of keys follows the schema.
    let keys: Vec<&String> = v.as_object().unwrap().keys().collect();
    assert_eq!(keys, ["customerId", "items", "shippingAddress"]);

    // Media-type example wins over schema synthesis.
    let customer: Value =
        serde_json::from_str(&spec.example_body(&r(Method::Post, "/customers")).unwrap()).unwrap();
    assert_eq!(customer["email"], "anna@example.com");
}

#[test]
fn schema_labels() {
    let spec = shop();
    assert_eq!(
        spec.response_schema_label(&r(Method::Post, "/orders"), 201)
            .as_deref(),
        Some("responses.201 → $ref Order")
    );
    assert_eq!(
        spec.response_schema_label(&r(Method::Get, "/products"), 200)
            .as_deref(),
        Some("responses.200 → object")
    );
    // Falls back to `default`.
    assert_eq!(
        spec.response_schema_label(&r(Method::Post, "/auth/refresh"), 500)
            .as_deref(),
        Some("responses.default → $ref Error")
    );
    // $ref'd response objects are followed.
    assert_eq!(
        spec.response_schema_label(&r(Method::Get, "/orders/ord_1"), 404)
            .as_deref(),
        Some("responses.404 → $ref Error")
    );
    // 204 has no body.
    assert_eq!(
        spec.response_schema_label(&r(Method::Delete, "/orders/{id}"), 204),
        None
    );
}

fn order(status: &str, quantity: Value) -> Value {
    json!({
        "id": "ord_8f2a",
        "status": status,
        "customerId": "cus_1001",
        "items": [
            { "productId": "prd_42", "quantity": quantity }
        ],
        "total": 999.8,
        "createdAt": "2026-10-15T12:30:00Z"
    })
}

#[test]
fn validate_correct_order() {
    let spec = shop();
    let m = spec.validate(&r(Method::Post, "/orders"), 201, &order("paid", json!(2)));
    assert_eq!(m, Some(vec![]));
}

#[test]
fn validate_mismatches_with_paths_and_lines() {
    let spec = shop();
    let body = order("new", json!("2"));
    let m = spec
        .validate(&r(Method::Post, "/orders"), 201, &body)
        .unwrap();
    assert_eq!(m.len(), 2, "{m:#?}");

    assert_eq!(m[0].path, "$.status");
    assert_eq!(m[0].title, "Значение не из enum");
    assert_eq!(
        m[0].expected,
        "\"pending\" | \"paid\" | \"shipped\" | \"cancelled\""
    );
    assert_eq!(m[0].actual, "\"new\"");
    assert_eq!(m[0].line, Some(2));

    assert_eq!(m[1].path, "$.items[0].quantity");
    assert_eq!(m[1].title, "Неверный тип");
    assert_eq!(m[1].expected, "integer");
    assert_eq!(m[1].line, Some(7));

    let pretty = serde_json::to_string_pretty(&body).unwrap();
    let lines: Vec<&str> = pretty.lines().collect();
    assert!(lines[2].contains("\"status\": \"new\""));
    assert!(lines[7].contains("\"quantity\": \"2\""));
    assert_eq!(line_of_path(&pretty, &body, "$.items[0].quantity"), Some(7));
}

#[test]
fn validate_missing_required_and_formats() {
    let spec = shop();
    let mut body = order("paid", json!(0));
    body.as_object_mut().unwrap().remove("total");
    body["createdAt"] = json!("вчера");
    let m = spec
        .validate(&r(Method::Get, "/orders/{id}"), 200, &body)
        .unwrap();
    let paths: Vec<(&str, &str)> = m
        .iter()
        .map(|m| (m.path.as_str(), m.title.as_str()))
        .collect();
    assert_eq!(
        paths,
        [
            ("$.total", "Отсутствует обязательное поле"),
            ("$.items[0].quantity", "Значение меньше минимума"),
            ("$.createdAt", "Неверный формат"),
        ]
    );
    // Missing field points at its parent object.
    assert_eq!(m[0].line, Some(0));
}

#[test]
fn validate_without_schema_is_none() {
    let spec = shop();
    assert_eq!(
        spec.validate(&r(Method::Delete, "/orders/{id}"), 204, &Value::Null),
        None
    );
    assert_eq!(
        spec.validate(&r(Method::Get, "/nope"), 200, &json!({})),
        None
    );
    assert_eq!(
        spec.validate(&r(Method::Get, "/orders/{id}"), 500, &json!({})),
        None
    );
}

#[test]
fn validate_all_of_and_pattern() {
    let spec = shop();
    let good = json!({
        "name": "Анна", "email": "anna@example.com", "phone": "+79001234567",
        "id": "3fa85f64-5717-4562-b3fc-2c963f66afa6", "createdAt": "2026-10-15T12:30:00Z"
    });
    assert_eq!(
        spec.validate(&r(Method::Post, "/customers"), 201, &good),
        Some(vec![])
    );
    let bad = json!({ "name": "Анна", "email": "nope", "phone": "123", "id": "x", "createdAt": "2026-10-15T12:30:00Z" });
    let m = spec
        .validate(&r(Method::Post, "/customers"), 201, &bad)
        .unwrap();
    let paths: Vec<&str> = m.iter().map(|m| m.path.as_str()).collect();
    assert_eq!(paths, ["$.email", "$.phone", "$.id"]);
}

#[test]
fn parses_json_spec() {
    let text = r##"{
      "openapi": "3.1.0",
      "info": { "title": "Mini", "version": "0.1" },
      "servers": [{ "url": "http://localhost:8080" }],
      "paths": {
        "/ping": {
          "post": {
            "operationId": "ping",
            "requestBody": { "content": { "application/json": { "schema": {
              "type": "object",
              "properties": {
                "count": { "type": "integer", "minimum": 5 },
                "ratio": { "type": "number", "exclusiveMinimum": 0 },
                "id": { "type": "string", "format": "uuid" },
                "when": { "type": "string", "format": "date-time" },
                "mail": { "type": "string", "format": "email" },
                "kind": { "type": "string", "enum": ["a", "b"] },
                "maybe": { "type": ["null", "boolean"] },
                "tags": { "type": "array", "items": { "type": "string" } }
              }
            } } } },
            "responses": { "2XX": { "description": "ok", "content": { "application/json": {
              "schema": { "type": "array", "items": { "$ref": "#/components/schemas/Pong" } } } } } }
          }
        },
        "/open": { "get": { "security": [], "responses": { "200": { "description": "ok" } } } }
      },
      "components": { "schemas": { "Pong": { "type": "object", "required": ["ok"], "properties": { "ok": { "type": "boolean" } } } } }
    }"##;
    let spec = Spec::parse(text).unwrap();
    assert_eq!(spec.title, "Mini");
    assert_eq!(spec.servers, vec!["http://localhost:8080"]);
    assert_eq!(spec.groups.len(), 1);
    assert_eq!(spec.groups[0].name, "default");
    let names: Vec<&str> = spec.groups[0]
        .endpoints
        .iter()
        .map(|e| e.name.as_str())
        .collect();
    assert_eq!(names, ["ping", "GET /open"]);
    assert!(spec.groups[0].endpoints.iter().all(|e| !e.requires_auth));

    let body: Value =
        serde_json::from_str(&spec.example_body(&r(Method::Post, "/ping")).unwrap()).unwrap();
    assert_eq!(
        body,
        json!({
            "count": 5,
            "ratio": 1,
            "id": "3fa85f64-5717-4562-b3fc-2c963f66afa6",
            "when": "2024-01-01T12:00:00Z",
            "mail": "user@example.com",
            "kind": "a",
            "maybe": true,
            "tags": ["string"]
        })
    );

    let ping = r(Method::Post, "/ping");
    assert_eq!(
        spec.response_schema_label(&ping, 204).as_deref(),
        Some("responses.2XX → array of $ref Pong")
    );
    let m = spec
        .validate(&ping, 200, &json!([{ "ok": true }, {}]))
        .unwrap();
    assert_eq!(m.len(), 1);
    assert_eq!(m[0].path, "$[1].ok");
    assert_eq!(m[0].line, Some(4));
}

#[test]
fn ref_recursion_is_guarded() {
    let text = r##"
openapi: 3.0.3
info: { title: Tree, version: "1" }
paths:
  /nodes:
    post:
      requestBody:
        content:
          application/json:
            schema: { $ref: '#/components/schemas/Node' }
      responses:
        '200':
          description: ok
          content:
            application/json:
              schema: { $ref: '#/components/schemas/Node' }
  /loop:
    get:
      responses:
        '200':
          description: ok
          content:
            application/json:
              schema: { $ref: '#/components/schemas/A' }
components:
  schemas:
    Node:
      type: object
      required: [name]
      properties:
        name: { type: string }
        parent: { $ref: '#/components/schemas/Node' }
        children:
          type: array
          items: { $ref: '#/components/schemas/Node' }
    A: { $ref: '#/components/schemas/B' }
    B: { $ref: '#/components/schemas/A' }
"##;
    let spec = Spec::parse(text).unwrap();
    let body: Value =
        serde_json::from_str(&spec.example_body(&r(Method::Post, "/nodes")).unwrap()).unwrap();
    assert_eq!(body, json!({ "name": "string", "children": [] }));

    // Deep recursive data validates fine and reports nested paths.
    let data =
        json!({ "name": "root", "children": [{ "name": "a", "children": [{ "children": [] }] }] });
    let m = spec
        .validate(&r(Method::Post, "/nodes"), 200, &data)
        .unwrap();
    assert_eq!(m.len(), 1);
    assert_eq!(m[0].path, "$.children[0].children[0].name");

    // A ↔ B cycle neither hangs nor overflows.
    assert_eq!(
        spec.validate(&r(Method::Get, "/loop"), 200, &json!(1)),
        Some(vec![])
    );
    assert_eq!(
        example_from_schema(
            spec.document(),
            &json!({ "$ref": "#/components/schemas/A" })
        ),
        Value::Null
    );
}
