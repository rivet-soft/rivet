//! UI-free core: data model, HTTP execution, OpenAPI, persistence.
//! Submodules are owned by separate workstreams; see PLAN.md.

pub mod curl;
pub mod http;
pub mod openapi;
pub(crate) mod openapi_refs;
pub mod postman;
pub mod rt;
pub mod runner;
pub mod schema;
pub mod script;
pub mod store;
pub mod types;
pub mod vars;
pub mod ws;

pub use types::*;
