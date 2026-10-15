use rivet::core::openapi::Spec;

#[test]
fn imports_local_path_references() {
    let temp = tempfile::tempdir().unwrap();
    let paths = temp.path().join("paths");
    let schemas = temp.path().join("schemas");
    std::fs::create_dir_all(&paths).unwrap();
    std::fs::create_dir_all(&schemas).unwrap();
    let root = temp.path().join("openapi.yaml");
    let path_item = paths.join("widgets.yaml");
    let model = schemas.join("Widget.yaml");
    let root_text = "openapi: 3.0.3\ninfo: {title: Widgets, version: 1.0.0}\npaths:\n  /widgets:\n    $ref: './paths/widgets.yaml'\ncomponents:\n  schemas:\n    Widget:\n      $ref: './schemas/Widget.yaml'\n";
    std::fs::write(&root, root_text).unwrap();
    std::fs::write(&path_item, "get:\n  summary: List widgets\n  operationId: listWidgets\n  responses:\n    '200':\n      description: OK\n      content:\n        application/json:\n          schema: { $ref: '../schemas/Widget.yaml' }\npost:\n  operationId: createWidget\n  requestBody:\n    content:\n      application/json:\n        schema: { $ref: '../schemas/Widget.yaml' }\n  responses:\n    '201': {description: Created}\n").unwrap();
    std::fs::write(&model, "type: object\nproperties:\n  id: {type: integer}\n").unwrap();

    let imported = Spec::load(&root).unwrap();
    assert_eq!(imported.endpoints().count(), 2);
    let names: Vec<_> = imported.endpoints().map(|e| e.name.clone()).collect();
    assert!(names.contains(&"List widgets".to_string()));
    let create = imported.find(rivet::core::Method::Post, "/widgets").unwrap();
    // The $ref'd request schema is resolved across files into an example body.
    assert!(create.example_body.as_deref().unwrap_or_default().contains("\"id\""));
}

#[test]
fn imports_a_user_spec_when_provided() {
    let Ok(path) = std::env::var("RELAY_REAL_SPEC") else {
        return;
    };
    let spec = Spec::load(path.as_ref()).unwrap();
    assert!(spec.endpoints().count() > 0);
}
