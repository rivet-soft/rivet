//! Collection mock servers (Docker + the engine from Settings → Моки):
//! start / stop / reload, an environment pointing at the mock, and stopping
//! everything when the app quits.

use std::path::PathBuf;

use gpui_kit::*;

use super::Workspace;
use crate::core::mock::{self, DockerStatus, MockRun};
use crate::core::{EnvColor, Environment, KeyValue};

/// A collection's mock, as the UI shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MockState {
    /// Checking Docker, pulling the image or waiting for the server.
    Starting,
    Running(MockRun),
    Failed(String),
}

impl Workspace {
    pub fn mock_state(&self, collection_id: &str) -> Option<&MockState> {
        self.mocks.get(collection_id)
    }

    /// Folder with the engine's files for a collection (next to project.json).
    fn mock_dir(&self, collection_id: &str) -> PathBuf {
        let base = self
            .store_path
            .as_ref()
            .and_then(|p| p.parent().map(|d| d.join("mocks")))
            .unwrap_or_else(|| std::env::temp_dir().join("rivet-mocks"));
        base.join(collection_id)
    }

    /// OpenAPI spec the collection's requests are linked to (answers requests
    /// that have no saved examples).
    fn mock_spec(&self, collection_id: &str) -> Option<std::sync::Arc<crate::core::openapi::Spec>> {
        self.linked_specs.get(collection_id).cloned().or_else(|| self.spec.clone())
    }

    fn mock_changed(&mut self, cx: &mut Context<Self>) {
        let mocks = self.mocks.clone();
        self.collection_panel.update(cx, |p, cx| p.set_mocks(mocks, cx));
        cx.notify();
    }

    /// "Запустить мок": checks Docker, picks a free port and starts the
    /// collection's mock in the background.
    pub fn start_mock(&mut self, collection_id: &str, cx: &mut Context<Self>) {
        if matches!(self.mocks.get(collection_id), Some(MockState::Starting | MockState::Running(_))) {
            return;
        }
        let Some(collection) = self.project.collections.iter().find(|c| c.id == collection_id).cloned() else {
            return;
        };
        let settings = self.project.settings.mock.clone();
        let dir = self.mock_dir(collection_id);
        let taken: Vec<u16> = self
            .mocks
            .values()
            .filter_map(|m| match m {
                MockState::Running(run) => Some(run.port),
                _ => None,
            })
            .collect();
        let id = collection_id.to_string();
        let spec = self.mock_spec(collection_id);
        self.mocks.insert(id.clone(), MockState::Starting);
        self.notify_user(format!("Запуск мока «{}»…", collection.name), cx);
        self.mock_changed(cx);
        let work = cx.background_spawn(async move {
            let docker = match mock::docker_status() {
                DockerStatus::Ready { docker, .. } => docker,
                DockerStatus::NotRunning { .. } => {
                    return Err("Docker не запущен — откройте Docker Desktop и повторите".to_string());
                }
                DockerStatus::Missing => {
                    return Err("Docker не найден: установите Docker Desktop (Настройки → Моки)".to_string());
                }
            };
            let port = mock::free_port(settings.port, &taken).ok_or("Нет свободного порта для мока")?;
            mock::start(&docker, &settings, &collection, spec.as_deref(), &dir, port)
        });
        let task = cx.spawn(async move |this, cx| {
            let result = work.await;
            this.update(cx, |this, cx| {
                let name = this
                    .project
                    .collections
                    .iter()
                    .find(|c| c.id == id)
                    .map(|c| c.name.clone())
                    .unwrap_or_default();
                match result {
                    Ok(run) => {
                        this.notify_user(format!("Мок «{name}» работает: {}", run.url), cx);
                        this.mocks.insert(id.clone(), MockState::Running(run));
                    }
                    Err(e) => {
                        this.notify_user(format!("Мок «{name}» не запустился: {e}"), cx);
                        this.mocks.insert(id.clone(), MockState::Failed(e));
                    }
                }
                this.mock_tasks.remove(&id);
                this.mock_changed(cx);
            })
            .ok();
        });
        self.mock_tasks.insert(collection_id.to_string(), task);
    }

    /// "Остановить мок".
    pub fn stop_mock(&mut self, collection_id: &str, cx: &mut Context<Self>) {
        self.mock_tasks.remove(collection_id);
        let Some(state) = self.mocks.remove(collection_id) else { return };
        self.mock_changed(cx);
        if let MockState::Running(run) = state {
            cx.background_spawn(async move {
                if let Some(docker) = mock::find_docker() {
                    let _ = mock::stop(&docker, &run);
                }
            })
            .detach();
            self.notify_user("Мок остановлен", cx);
        }
    }

    /// "Обновить": rewrites a running mock's stubs from the collection.
    pub fn reload_mock(&mut self, collection_id: &str, cx: &mut Context<Self>) {
        let Some(MockState::Running(run)) = self.mocks.get(collection_id).cloned() else { return };
        let Some(collection) = self.project.collections.iter().find(|c| c.id == collection_id).cloned() else {
            return;
        };
        let id = collection_id.to_string();
        let spec = self.mock_spec(collection_id);
        let work = cx.background_spawn(async move { mock::reload(&run, &collection, spec.as_deref()) });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(run) => {
                        if matches!(this.mocks.get(&id), Some(MockState::Running(_))) {
                            this.mocks.insert(id.clone(), MockState::Running(run));
                        }
                    }
                    Err(e) => this.notify_user(format!("Мок не обновился: {e}"), cx),
                }
                this.mock_changed(cx);
            })
            .ok();
        })
        .detach();
    }

    /// After examples or requests of a collection change: refresh its mock.
    pub(super) fn reload_mock_if_running(&mut self, collection_id: Option<&str>, cx: &mut Context<Self>) {
        if let Some(id) = collection_id {
            if matches!(self.mocks.get(id), Some(MockState::Running(_))) {
                self.reload_mock(id, cx);
            }
        }
    }

    /// "Окружение Mock": an environment whose baseUrl is the mock, made active.
    pub fn create_mock_environment(&mut self, collection_id: &str, cx: &mut Context<Self>) {
        let Some(MockState::Running(run)) = self.mocks.get(collection_id) else { return };
        let name = self
            .project
            .collections
            .iter()
            .find(|c| c.id == collection_id)
            .map(|c| format!("Mock · {}", c.name))
            .unwrap_or_else(|| "Mock".into());
        let url = run.url.clone();
        let ix = match self.project.environments.iter().position(|e| e.name == name) {
            Some(ix) => {
                let env = &mut self.project.environments[ix];
                env.vars.retain(|v| v.key != "baseUrl");
                env.vars.insert(0, KeyValue::new("baseUrl", url));
                ix
            }
            None => {
                let mut env = Environment::new(name.clone(), EnvColor::Purple);
                env.vars.push(KeyValue::new("baseUrl", url));
                self.project.environments.push(env);
                self.project.environments.len() - 1
            }
        };
        self.project.active_env = ix;
        self.env_panel_stale = true;
        self.schedule_save(cx);
        self.notify_user(format!("Окружение «{name}» активно"), cx);
        cx.notify();
    }

    /// App quit: stop every running mock container (blocking, short timeouts).
    pub(super) fn stop_all_mocks(&mut self) {
        let runs: Vec<MockRun> = self
            .mocks
            .drain()
            .filter_map(|(_, m)| match m {
                MockState::Running(run) => Some(run),
                _ => None,
            })
            .collect();
        if runs.is_empty() {
            return;
        }
        if let Some(docker) = mock::find_docker() {
            for run in runs {
                let _ = mock::stop(&docker, &run);
            }
        }
    }
}
