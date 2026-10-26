//! UI-free core: data model, HTTP execution, OpenAPI, persistence.
//! Submodules are owned by separate workstreams; see PLAN.md.

pub mod bulk;
pub mod codegen;
pub mod collection_run;
pub mod curl;
pub mod export;
pub mod har;
pub mod header_check;
pub mod http;
pub mod import;
pub mod mcp_activity;
pub mod mcp_clients;
pub mod mock;
pub mod openapi;
pub(crate) mod openapi_refs;
pub mod postman;
pub mod project_ops;
pub mod rt;
pub mod runner;
pub mod schema;
pub mod schema_hover;
pub mod script;
pub mod secrets;
pub mod store;
pub mod tree_ops;
pub mod types;
pub mod update;
pub mod vars;
pub mod ws;

pub use types::*;
