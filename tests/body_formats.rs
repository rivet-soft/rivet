use rivet::core::postman::PostmanImport;
use rivet::core::{Body, BodyKind, KeyValue, Method, RequestSpec, SendError, curl, vars};

#[test]
fn multipart_sends_text_and_file_bytes_with_boundary() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("sample.bin");
    std::fs::write(&file, [0, 1, 255, b'X']).unwrap();
    let mut file_field = KeyValue::new("attachment", file.display().to_string());
    file_field.file = true;
    let mut request = RequestSpec::new("upload", Method::Post, "https://example.test/upload");
    request.body = Body {
        kind: BodyKind::Multipart,
        form: vec![KeyValue::new("note", "hello"), file_field],
        ..Body::default()
    };
    let resolved = vars::resolve(&request, None).unwrap();
    let content_type = resolved
        .headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("content-type"))
        .unwrap()
        .1
        .clone();
    assert!(content_type.starts_with("multipart/form-data; boundary=rivet-"));
    let boundary = content_type.split("boundary=").nth(1).unwrap();
    let bytes = resolved.body.unwrap();
    assert!(
        bytes
            .windows(b"name=\"note\"\r\n\r\nhello".len())
            .any(|w| w == b"name=\"note\"\r\n\r\nhello")
    );
    assert!(
        bytes
            .windows(b"filename=\"sample.bin\"".len())
            .any(|w| w == b"filename=\"sample.bin\"")
    );
    assert!(bytes.windows(4).any(|w| w == [0, 1, 255, b'X']));
    assert!(bytes.ends_with(format!("--{boundary}--\r\n").as_bytes()));

    let resolved = vars::resolve(&request, None).unwrap();
    let command = curl::to_curl_with_source(&request, &resolved, None);
    assert!(command.contains("-F 'note=hello'"));
    assert!(command.contains(&format!("-F 'attachment=@{}'", file.display())));
    assert!(!command.contains("boundary=rivet-"));
}

#[test]
fn binary_sends_exact_file_and_reports_missing_file() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("data.png");
    std::fs::write(&file, [137, 80, 78, 71, 0]).unwrap();
    let mut request = RequestSpec::new("binary", Method::Post, "https://example.test/file");
    request.body = Body {
        kind: BodyKind::Binary,
        file_path: Some(file.clone()),
        ..Body::default()
    };
    let resolved = vars::resolve(&request, None).unwrap();
    assert_eq!(resolved.body.as_deref().unwrap(), [137, 80, 78, 71, 0]);
    assert!(
        resolved
            .headers
            .contains(&("Content-Type".into(), "image/png".into()))
    );
    let command = curl::to_curl_with_source(&request, &resolved, None);
    assert!(command.contains(&format!("--data-binary '@{}'", file.display())));
    std::fs::remove_file(file).unwrap();
    assert!(matches!(
        vars::resolve(&request, None),
        Err(SendError::File { .. })
    ));
}

#[test]
fn postman_file_fields_resolve_relative_to_collection_file() {
    let dir = tempfile::tempdir().unwrap();
    let json = r#"{"info":{"name":"Files","schema":"https://schema.getpostman.com/json/collection/v2.1.0/collection.json"},"item":[{"name":"Upload","request":{"method":"POST","url":"https://example.test/upload","body":{"mode":"formdata","formdata":[{"key":"file","type":"file","src":"upload.bin"},{"key":"label","value":"x"}]}}}]}"#;
    let mut imported = PostmanImport::parse(json).unwrap();
    imported.resolve_file_paths(&dir.path().join("collection.json"));
    let req = &imported.collection.requests[0];
    assert_eq!(req.body.kind, BodyKind::Multipart);
    assert_eq!(
        req.body.form[0].value,
        dir.path().join("upload.bin").display().to_string()
    );
    assert!(req.body.form[0].file);
}
