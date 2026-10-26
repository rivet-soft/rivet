//! UI-free import of OpenAPI/Swagger specifications and Postman collections
//! into a new [`Collection`] (same rules as the window's «Импорт»).

use std::path::Path;

use super::export;
use super::openapi::Spec;
use super::postman::{self, PostmanImport};
use super::types::Collection;

/// What [`import_file`] produced.
pub struct ImportOutcome {
    pub collection: Collection,
    /// "openapi", "postman" or "rivet".
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

/// Reads a native Rivet export (`*.rivet.json`). A collection linked to an
/// OpenAPI file keeps the link only while that file still loads.
pub fn rivet_collection(doc: &serde_json::Value) -> Result<(Collection, Option<Spec>), String> {
    let mut collection = export::from_rivet(doc)?;
    let spec = collection.source.as_deref().and_then(|p| Spec::load(p).ok());
    if spec.is_none() {
        collection.source = None;
    }
    Ok((collection, spec))
}

/// Imports a Rivet export, a Postman v2.x collection or an OpenAPI 3 / Swagger 2 file.
pub fn import_file(path: &Path) -> Result<ImportOutcome, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let doc = serde_json::from_str::<serde_json::Value>(&text).ok();
    if let Some(doc) = doc.as_ref().filter(|d| export::is_rivet(d)) {
        let (collection, spec) = rivet_collection(doc)?;
        return Ok(ImportOutcome { collection, format: "rivet", spec, warnings: Vec::new() });
    }
    let is_postman = doc.is_some_and(|doc| postman::is_postman(&doc));
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
