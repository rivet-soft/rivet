//! UI-free import of OpenAPI/Swagger specifications and Postman collections
//! into a new [`Collection`] (same rules as the window's «Импорт»).

use std::path::Path;

use super::openapi::Spec;
use super::postman::{self, PostmanImport};
use super::types::Collection;

/// What [`import_file`] produced.
pub struct ImportOutcome {
    pub collection: Collection,
    /// "openapi" or "postman".
    pub format: &'static str,
    /// The parsed spec for OpenAPI imports (the collection is linked to the file).
    pub spec: Option<Spec>,
    /// Human-readable notes about items that need manual attention.
    pub warnings: Vec<String>,
}

/// A collection linked to `path` with one request per spec operation, a folder
/// per tag and the spec's server as `baseUrl`.
pub fn collection_from_spec(spec: &Spec, path: &Path) -> Collection {
    let mut collection = Collection::new(&spec.title);
    collection.source = Some(path.to_path_buf());
    collection.vars = spec.collection_variables();
    for group in &spec.groups {
        collection.folders.push(group.name.clone());
        for endpoint in &group.endpoints {
            let mut request = endpoint.to_request();
            request.collection_id = Some(collection.id.clone());
            request.folder = Some(group.name.clone());
            collection.requests.push(request);
        }
    }
    collection
}

/// Imports a Postman v2.x collection or an OpenAPI 3 / Swagger 2 file.
pub fn import_file(path: &Path) -> Result<ImportOutcome, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let is_postman = serde_json::from_str::<serde_json::Value>(&text)
        .ok()
        .is_some_and(|doc| postman::is_postman(&doc));
    if is_postman {
        let mut imported = PostmanImport::parse(&text)?;
        imported.resolve_file_paths(path);
        let mut warnings = Vec::new();
        if imported.skipped > 0 {
            warnings.push(format!("{} items skipped", imported.skipped));
        }
        if imported.unsupported_bodies > 0 {
            warnings.push(format!("{} bodies need manual setup", imported.unsupported_bodies));
        }
        if imported.unsupported_auth > 0 {
            warnings.push(format!("{} auth settings need manual setup", imported.unsupported_auth));
        }
        let collection = imported.collection;
        return Ok(ImportOutcome { collection, format: "postman", spec: None, warnings });
    }
    let spec = Spec::load(path)?;
    let collection = collection_from_spec(&spec, path);
    Ok(ImportOutcome { collection, format: "openapi", spec: Some(spec), warnings: Vec::new() })
}
