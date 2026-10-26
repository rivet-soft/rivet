//! Duplicate / copy / cut / paste of requests, folders and collections (the
//! sidebar "···" menus). Folders are `" / "`-separated paths inside a
//! collection; a folder's subtree is every folder path and request below it.

use super::types::{Collection, Project, RequestSpec, new_id};

/// What the sidebar clipboard holds.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ClipItem {
    Request { collection_id: String, request_id: String },
    Folder { collection_id: String, path: String },
}

/// Clipboard entry: copied, or cut (moved on paste).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Clip {
    pub item: ClipItem,
    pub cut: bool,
}

/// What a paste did, for the toast and for updating open tabs.
#[derive(Debug, PartialEq, Eq)]
pub struct Pasted {
    /// Moved requests: (id, new collection id, new folder).
    pub moved: Vec<(String, String, Option<String>)>,
    /// Ids of newly created requests (copies).
    pub created: Vec<String>,
    pub message: String,
}

fn in_subtree(path: &str, root: &str) -> bool {
    path == root || path.starts_with(&format!("{root} / "))
}

fn last_segment(path: &str) -> &str {
    path.rsplit(" / ").next().unwrap_or(path)
}

fn join(parent: Option<&str>, name: &str) -> String {
    match parent.filter(|p| !p.is_empty()) {
        Some(p) => format!("{p} / {name}"),
        None => name.to_string(),
    }
}

fn parent_of(path: &str) -> Option<&str> {
    path.rsplit_once(" / ").map(|(p, _)| p)
}

/// `name`, or `name (копия)`, `name (копия 2)`… — the first one `taken` rejects not.
fn copy_name(name: &str, taken: impl Fn(&str) -> bool, force_suffix: bool) -> String {
    if !force_suffix && !taken(name) {
        return name.to_string();
    }
    let first = format!("{name} (копия)");
    if !taken(&first) {
        return first;
    }
    (2..)
        .map(|n| format!("{name} (копия {n})"))
        .find(|candidate| !taken(candidate))
        .unwrap()
}

fn folder_exists(c: &Collection, path: &str) -> bool {
    c.folders.iter().any(|f| f == path)
        || c.requests.iter().any(|r| r.folder.as_deref().is_some_and(|f| in_subtree(f, path)))
}

/// Re-roots `path` (inside the subtree at `from`) under `to`.
fn reroot(path: &str, from: &str, to: &str) -> String {
    format!("{to}{}", &path[from.len()..])
}

fn fresh_copy(request: &RequestSpec, collection_id: &str, same_collection: bool) -> RequestSpec {
    let mut copy = request.clone();
    copy.id = new_id();
    copy.collection_id = Some(collection_id.to_string());
    if !same_collection {
        // The OpenAPI link belongs to the source collection's spec.
        copy.spec_ref = None;
    }
    copy
}

/// Copies request `request_id` right after itself as "Name (копия)". Returns
/// the new request's id.
pub fn duplicate_request(c: &mut Collection, request_id: &str) -> Option<String> {
    let ix = c.requests.iter().position(|r| r.id == request_id)?;
    let mut copy = fresh_copy(&c.requests[ix], &c.id.clone(), true);
    copy.name = copy_name(&c.requests[ix].name, |n| c.requests.iter().any(|r| r.name == n), true);
    let id = copy.id.clone();
    c.requests.insert(ix + 1, copy);
    Some(id)
}

/// Copies the folder at `path` (subfolders and requests, fresh ids) as a
/// sibling "Name (копия)". Returns the new folder path.
pub fn duplicate_folder(c: &mut Collection, path: &str) -> Option<String> {
    if !folder_exists(c, path) {
        return None;
    }
    let name = copy_name(last_segment(path), |n| folder_exists(c, &join(parent_of(path), n)), true);
    let dest = join(parent_of(path), &name);
    copy_subtree_within(c, path, &dest);
    Some(dest)
}

/// Copies a folder subtree inside one collection from `from` to `to`.
fn copy_subtree_within(c: &mut Collection, from: &str, to: &str) {
    let folders: Vec<String> = c
        .folders
        .iter()
        .filter(|f| in_subtree(f, from))
        .map(|f| reroot(f, from, to))
        .collect();
    let id = c.id.clone();
    let requests: Vec<RequestSpec> = c
        .requests
        .iter()
        .filter(|r| r.folder.as_deref().is_some_and(|f| in_subtree(f, from)))
        .map(|r| {
            let mut copy = fresh_copy(r, &id, true);
            copy.folder = r.folder.as_deref().map(|f| reroot(f, from, to));
            copy
        })
        .collect();
    if !c.folders.iter().any(|f| f == to) {
        c.folders.push(to.to_string());
    }
    for f in folders {
        if !c.folders.contains(&f) {
            c.folders.push(f);
        }
    }
    c.requests.extend(requests);
}

/// A copy of the whole collection: "Name (копия)", fresh collection and
/// request ids, same folders, variables and settings.
pub fn duplicate_collection(project: &Project, collection_id: &str) -> Option<Collection> {
    let source = project.collections.iter().find(|c| c.id == collection_id)?;
    let mut copy = source.clone();
    copy.id = new_id();
    copy.name = copy_name(&source.name, |n| project.collections.iter().any(|c| c.name == n), true);
    for r in &mut copy.requests {
        r.id = new_id();
        r.collection_id = Some(copy.id.clone());
    }
    Some(copy)
}

/// Pastes `clip` into collection `dest_collection` (root, or folder
/// `dest_folder`). A copy gets fresh ids; a cut moves the originals (ids kept,
/// so open tabs stay linked).
pub fn paste(
    project: &mut Project,
    clip: &Clip,
    dest_collection: &str,
    dest_folder: Option<&str>,
) -> Result<Pasted, String> {
    let dest_folder = dest_folder.filter(|f| !f.is_empty());
    let di = project
        .collections
        .iter()
        .position(|c| c.id == dest_collection)
        .ok_or("Коллекция для вставки не найдена")?;
    match &clip.item {
        ClipItem::Request { collection_id, request_id } => {
            let si = project
                .collections
                .iter()
                .position(|c| &c.id == collection_id)
                .ok_or("Исходная коллекция не найдена")?;
            let ri = project.collections[si]
                .requests
                .iter()
                .position(|r| &r.id == request_id)
                .ok_or("Скопированный запрос больше не существует")?;
            let same = si == di;
            if clip.cut {
                let mut request = project.collections[si].requests.remove(ri);
                request.collection_id = Some(dest_collection.to_string());
                request.folder = dest_folder.map(str::to_string);
                if !same {
                    request.spec_ref = None;
                }
                let moved = vec![(request.id.clone(), dest_collection.to_string(), request.folder.clone())];
                let message = format!("Запрос «{}» перемещён", request.name);
                project.collections[di].requests.push(request);
                Ok(Pasted { moved, created: Vec::new(), message })
            } else {
                let source = project.collections[si].requests[ri].clone();
                let mut copy = fresh_copy(&source, dest_collection, same);
                copy.folder = dest_folder.map(str::to_string);
                let dest = &project.collections[di];
                let taken = |n: &str| {
                    dest.requests.iter().any(|r| r.name == n && r.folder.as_deref() == dest_folder)
                };
                copy.name = copy_name(&source.name, taken, false);
                let created = vec![copy.id.clone()];
                let message = format!("Запрос «{}» вставлен", copy.name);
                project.collections[di].requests.push(copy);
                Ok(Pasted { moved: Vec::new(), created, message })
            }
        }
        ClipItem::Folder { collection_id, path } => {
            let si = project
                .collections
                .iter()
                .position(|c| &c.id == collection_id)
                .ok_or("Исходная коллекция не найдена")?;
            if !folder_exists(&project.collections[si], path) {
                return Err("Скопированная папка больше не существует".into());
            }
            if si == di && dest_folder.is_some_and(|d| in_subtree(d, path)) {
                return Err("Нельзя вставить папку в неё саму".into());
            }
            if clip.cut && si == di && parent_of(path) == dest_folder {
                return Ok(Pasted { moved: Vec::new(), created: Vec::new(), message: "Папка уже здесь".into() });
            }
            let dest_c = &project.collections[di];
            let name = copy_name(last_segment(path), |n| folder_exists(dest_c, &join(dest_folder, n)), false);
            let to = join(dest_folder, &name);
            let same = si == di;
            let source = project.collections[si].clone();
            let folders: Vec<String> = source
                .folders
                .iter()
                .filter(|f| in_subtree(f, path))
                .map(|f| reroot(f, path, &to))
                .collect();
            let subtree: Vec<&RequestSpec> = source
                .requests
                .iter()
                .filter(|r| r.folder.as_deref().is_some_and(|f| in_subtree(f, path)))
                .collect();
            let mut moved = Vec::new();
            let mut created = Vec::new();
            let mut requests = Vec::new();
            for r in &subtree {
                let folder = r.folder.as_deref().map(|f| reroot(f, path, &to));
                if clip.cut {
                    let mut m = (*r).clone();
                    m.collection_id = Some(dest_collection.to_string());
                    m.folder = folder.clone();
                    if !same {
                        m.spec_ref = None;
                    }
                    moved.push((m.id.clone(), dest_collection.to_string(), folder));
                    requests.push(m);
                } else {
                    let mut copy = fresh_copy(r, dest_collection, same);
                    copy.folder = folder;
                    created.push(copy.id.clone());
                    requests.push(copy);
                }
            }
            if clip.cut {
                let src = &mut project.collections[si];
                src.folders.retain(|f| !in_subtree(f, path));
                src.requests.retain(|r| !r.folder.as_deref().is_some_and(|f| in_subtree(f, path)));
            }
            let dest = &mut project.collections[di];
            if !dest.folders.contains(&to) {
                dest.folders.push(to.clone());
            }
            for f in folders {
                if !dest.folders.contains(&f) {
                    dest.folders.push(f);
                }
            }
            dest.requests.extend(requests);
            let message = if clip.cut {
                format!("Папка «{name}» перемещена")
            } else {
                format!("Папка «{name}» вставлена")
            };
            Ok(Pasted { moved, created, message })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::{Method, SpecRef};

    fn req(c: &Collection, name: &str, folder: Option<&str>) -> RequestSpec {
        let mut r = RequestSpec::new(name, Method::Get, format!("http://h/{name}"));
        r.collection_id = Some(c.id.clone());
        r.folder = folder.map(str::to_string);
        r
    }

    fn project() -> Project {
        let mut p = crate::core::store::seed_project();
        let mut a = Collection::new("A");
        a.folders = vec!["Users".into(), "Users / Admin".into(), "Empty".into()];
        let list = req(&a, "List", Some("Users"));
        let mut admin = req(&a, "Ban", Some("Users / Admin"));
        admin.spec_ref = Some(SpecRef { method: Method::Get, path: "/ban".into() });
        let root = req(&a, "Root", None);
        a.requests = vec![list, admin, root];
        p.collections = vec![a, Collection::new("B")];
        p
    }

    #[test]
    fn duplicates() {
        let mut p = project();
        let list_id = p.collections[0].requests[0].id.clone();
        let id = duplicate_request(&mut p.collections[0], &list_id).unwrap();
        let c = &p.collections[0];
        assert_eq!(c.requests[1].id, id, "right after the original");
        assert_eq!(c.requests[1].name, "List (копия)");
        assert_eq!(c.requests[1].folder.as_deref(), Some("Users"));
        let id2 = duplicate_request(&mut p.collections[0], &list_id).unwrap();
        assert_eq!(p.collections[0].requests.iter().find(|r| r.id == id2).unwrap().name, "List (копия 2)");

        let path = duplicate_folder(&mut p.collections[0], "Users").unwrap();
        assert_eq!(path, "Users (копия)");
        let c = &p.collections[0];
        assert!(c.folders.contains(&"Users (копия) / Admin".to_string()));
        let copied: Vec<&RequestSpec> = c.requests.iter().filter(|r| r.folder.as_deref().is_some_and(|f| f.starts_with("Users (копия)"))).collect();
        assert_eq!(copied.len(), 4, "List, its two copies and Ban");
        let mut ids: Vec<&str> = c.requests.iter().map(|r| r.id.as_str()).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), c.requests.len(), "copies get fresh ids");
        assert!(duplicate_folder(&mut p.collections[0], "Nope").is_none());

        let dup = duplicate_collection(&p, &p.collections[0].id).unwrap();
        assert_eq!(dup.name, "A (копия)");
        assert_ne!(dup.id, p.collections[0].id);
        assert!(dup.requests.iter().all(|r| r.collection_id.as_deref() == Some(dup.id.as_str())));
        assert!(dup.requests.iter().zip(&p.collections[0].requests).all(|(a, b)| a.id != b.id && a.name == b.name));
    }

    #[test]
    fn copy_and_cut_requests() {
        let mut p = project();
        let (a, b) = (p.collections[0].id.clone(), p.collections[1].id.clone());
        let ban = p.collections[0].requests[1].id.clone();
        let clip = Clip { item: ClipItem::Request { collection_id: a.clone(), request_id: ban.clone() }, cut: false };
        let pasted = paste(&mut p, &clip, &b, None).unwrap();
        let copy = &p.collections[1].requests[0];
        assert_eq!(pasted.created, vec![copy.id.clone()]);
        assert_ne!(copy.id, ban);
        assert_eq!(copy.name, "Ban");
        assert_eq!(copy.spec_ref, None, "spec link dropped in another collection");
        assert_eq!(copy.collection_id.as_deref(), Some(b.as_str()));
        // Same place twice: the name gets a suffix.
        paste(&mut p, &clip, &b, None).unwrap();
        assert_eq!(p.collections[1].requests[1].name, "Ban (копия)");

        let cut = Clip { cut: true, ..clip };
        let pasted = paste(&mut p, &cut, &a, Some("Users")).unwrap();
        assert_eq!(pasted.moved, vec![(ban.clone(), a.clone(), Some("Users".into()))]);
        let moved = p.collections[0].requests.iter().find(|r| r.id == ban).unwrap();
        assert_eq!(moved.folder.as_deref(), Some("Users"));
        assert!(moved.spec_ref.is_some(), "same collection keeps the link");
        assert_eq!(p.collections[0].requests.iter().filter(|r| r.id == ban).count(), 1);
        assert!(paste(&mut p, &cut, "nope", None).is_err());
    }

    #[test]
    fn copy_and_cut_folders() {
        let mut p = project();
        let (a, b) = (p.collections[0].id.clone(), p.collections[1].id.clone());
        let clip = Clip { item: ClipItem::Folder { collection_id: a.clone(), path: "Users".into() }, cut: false };
        let pasted = paste(&mut p, &clip, &b, None).unwrap();
        assert_eq!(pasted.created.len(), 2);
        assert_eq!(p.collections[1].folders, ["Users", "Users / Admin"]);
        assert!(p.collections[1].requests.iter().all(|r| r.collection_id.as_deref() == Some(b.as_str())));
        // Into a folder of the same collection; into itself is refused.
        assert!(paste(&mut p, &clip, &a, Some("Users / Admin")).is_err());
        let pasted = paste(&mut p, &clip, &a, Some("Empty")).unwrap();
        assert!(pasted.message.contains("Users"));
        assert!(p.collections[0].folders.contains(&"Empty / Users / Admin".to_string()));
        assert!(p.collections[0].requests.iter().any(|r| r.folder.as_deref() == Some("Empty / Users / Admin")));

        // Cut moves everything and keeps ids.
        let ids: Vec<String> = p.collections[0].requests.iter().filter(|r| r.folder.as_deref().is_some_and(|f| in_subtree(f, "Users"))).map(|r| r.id.clone()).collect();
        let cut = Clip { cut: true, ..clip };
        let pasted = paste(&mut p, &cut, &b, None).unwrap();
        assert_eq!(pasted.moved.len(), ids.len());
        assert!(pasted.moved.iter().all(|(id, c, _)| ids.contains(id) && c == &b));
        assert!(!p.collections[0].folders.iter().any(|f| in_subtree(f, "Users")));
        assert!(p.collections[0].folders.contains(&"Empty / Users".to_string()), "a same-named nested folder is not the cut one");
        assert!(p.collections[1].folders.contains(&"Users (копия) / Admin".to_string()), "B already had Users");
        assert!(paste(&mut p, &cut, &b, None).is_err(), "the cut folder is gone from A");
    }
}
