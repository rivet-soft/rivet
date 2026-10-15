//! Resolve local OpenAPI `$ref` files for reading, while retaining the source
//! documents so the editor can write changes back to their original files.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

use super::openapi::yaml_to_json;

#[derive(Clone, Debug)]
pub(crate) struct ExternalSource {
    pub document: Value,
}

#[derive(Clone, Debug)]
pub(crate) struct ResolvedDocument {
    pub document: Value,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) struct RefTarget {
    pub file: PathBuf,
    pub pointer: String,
}

pub(crate) fn target_for(base_file: &Path, reference: &str) -> Result<Option<RefTarget>, String> {
    let (file, fragment) = reference.split_once('#').unwrap_or((reference, ""));
    if file.is_empty() || file.contains("://") || file.starts_with("urn:") {
        return Ok(None);
    }
    let base = base_file
        .parent()
        .ok_or("Не удалось определить каталог спецификации")?;
    let path = base.join(file).canonicalize().map_err(|e| {
        format!(
            "Не удалось открыть ссылку {reference} из {}: {e}",
            base_file.display()
        )
    })?;
    if !fragment.is_empty() && !fragment.starts_with('/') {
        return Err(format!("Неподдерживаемый фрагмент ссылки: {reference}"));
    }
    Ok(Some(RefTarget {
        file: path,
        pointer: fragment.to_string(),
    }))
}

pub(crate) fn resolve(
    root_path: &Path,
    raw: &Value,
    overrides: Option<&HashMap<PathBuf, Value>>,
) -> Result<ResolvedDocument, String> {
    let root_path = root_path.canonicalize().map_err(|e| e.to_string())?;
    let mut aliases = HashMap::new();
    if let Some(schemas) = raw
        .pointer("/components/schemas")
        .and_then(Value::as_object)
    {
        for (name, schema) in schemas {
            if let Some(reference) = schema.get("$ref").and_then(Value::as_str) {
                if let Some(target) = target_for(&root_path, reference)? {
                    aliases.entry(target).or_insert_with(|| name.clone());
                }
            }
        }
    }
    let mut resolver = Resolver {
        aliases,
        overrides,
        sources: HashMap::new(),
        active: HashSet::new(),
    };
    let mut document = raw.clone();
    let Some(root) = raw.as_object() else {
        return Err("Спецификация должна быть объектом".into());
    };
    for (key, value) in root {
        if key == "components" {
            let mut components = value.clone();
            if let Some(schemas) = value.get("schemas").and_then(Value::as_object) {
                let mut resolved_schemas = Map::new();
                for (name, schema) in schemas {
                    resolved_schemas.insert(
                        name.clone(),
                        resolver.resolve_value(schema, &root_path, Some(name), 0)?,
                    );
                }
                if let Some(obj) = components.as_object_mut() {
                    obj.insert("schemas".into(), Value::Object(resolved_schemas));
                }
            }
            if let Some(obj) = document.as_object_mut() {
                obj.insert(key.clone(), components);
            }
        } else {
            let resolved = resolver.resolve_value(value, &root_path, None, 0)?;
            if let Some(obj) = document.as_object_mut() {
                obj.insert(key.clone(), resolved);
            }
        }
    }
    Ok(ResolvedDocument { document })
}

struct Resolver<'a> {
    aliases: HashMap<RefTarget, String>,
    overrides: Option<&'a HashMap<PathBuf, Value>>,
    sources: HashMap<PathBuf, ExternalSource>,
    active: HashSet<RefTarget>,
}

impl Resolver<'_> {
    fn source(&mut self, path: &Path) -> Result<Value, String> {
        if let Some(source) = self.sources.get(path) {
            return Ok(source.document.clone());
        }
        let original = std::fs::read_to_string(path)
            .map_err(|e| format!("Не удалось прочитать {}: {e}", path.display()))?;
        let document = parse_fragment(&original)?;
        let document = self
            .overrides
            .and_then(|m| m.get(path))
            .cloned()
            .unwrap_or(document);
        self.sources.insert(
            path.to_path_buf(),
            ExternalSource {
                document: document.clone(),
            },
        );
        Ok(document)
    }

    fn resolve_value(
        &mut self,
        value: &Value,
        base_file: &Path,
        expand_alias: Option<&str>,
        depth: usize,
    ) -> Result<Value, String> {
        if depth > 64 {
            return Err(format!(
                "Слишком глубокая цепочка $ref около {}",
                base_file.display()
            ));
        }
        match value {
            Value::Object(object) => {
                if let Some(reference) = object.get("$ref").and_then(Value::as_str) {
                    if let Some(target) = target_for(base_file, reference)? {
                        if let Some(name) = self.aliases.get(&target) {
                            if expand_alias != Some(name.as_str()) {
                                return Ok(json!({"$ref": format!("#/components/schemas/{name}")}));
                            }
                        }
                        if !self.active.insert(target.clone()) {
                            return Ok(value.clone());
                        }
                        let source = self.source(&target.file)?;
                        let selected = if target.pointer.is_empty() {
                            &source
                        } else {
                            source
                                .pointer(&target.pointer)
                                .ok_or_else(|| format!("Фрагмент {reference} не найден"))?
                        };
                        let mut resolved =
                            self.resolve_value(selected, &target.file, None, depth + 1)?;
                        self.active.remove(&target);
                        if let Some(resolved_object) = resolved.as_object_mut() {
                            for (key, sibling) in
                                object.iter().filter(|(key, _)| key.as_str() != "$ref")
                            {
                                resolved_object.insert(
                                    key.clone(),
                                    self.resolve_value(sibling, base_file, None, depth + 1)?,
                                );
                            }
                        }
                        return Ok(resolved);
                    }
                }
                let mut out = Map::new();
                for (key, child) in object {
                    out.insert(
                        key.clone(),
                        self.resolve_value(child, base_file, None, depth + 1)?,
                    );
                }
                Ok(Value::Object(out))
            }
            Value::Array(items) => Ok(Value::Array(
                items
                    .iter()
                    .map(|v| self.resolve_value(v, base_file, None, depth + 1))
                    .collect::<Result<_, _>>()?,
            )),
            _ => Ok(value.clone()),
        }
    }
}

fn parse_fragment(text: &str) -> Result<Value, String> {
    let text = text.trim_start_matches('\u{feff}');
    if text.trim_start().starts_with('{') {
        serde_json::from_str(text).map_err(|e| format!("Некорректный JSON: {e}"))
    } else {
        let value = serde_yaml::from_str(text).map_err(|e| format!("Некорректный YAML: {e}"))?;
        Ok(yaml_to_json(value))
    }
}
