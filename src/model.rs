//! Static demo data mirroring the design mock until real collections are loaded.

use gpui_kit::{Hsla, SharedString};

use crate::theme::Palette;

#[derive(
    Clone, Copy, PartialEq, Eq, Debug, Hash, Default, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "UPPERCASE")]
pub enum Method {
    #[default]
    Get,
    Post,
    Put,
    Patch,
    Delete,
    Head,
    Options,
}

impl Method {
    pub const ALL: [Method; 7] = [
        Method::Get,
        Method::Post,
        Method::Put,
        Method::Patch,
        Method::Delete,
        Method::Head,
        Method::Options,
    ];

    pub fn parse(s: &str) -> Option<Method> {
        Self::ALL
            .into_iter()
            .find(|m| m.full_label().eq_ignore_ascii_case(s))
    }

    pub fn label(self) -> &'static str {
        match self {
            Method::Get => "GET",
            Method::Post => "POST",
            Method::Put => "PUT",
            Method::Patch => "PATCH",
            Method::Delete => "DEL",
            Method::Head => "HEAD",
            Method::Options => "OPT",
        }
    }

    pub fn full_label(self) -> &'static str {
        match self {
            Method::Delete => "DELETE",
            Method::Options => "OPTIONS",
            m => m.label(),
        }
    }

    pub fn color(self, p: &Palette) -> Hsla {
        match self {
            Method::Get => p.online,
            Method::Post => p.accent,
            Method::Put => p.amber,
            Method::Patch => p.purple,
            Method::Delete => p.danger,
            Method::Head => p.teal,
            Method::Options => p.text_dim,
        }
    }
}

#[derive(Clone)]
pub struct Endpoint {
    pub method: Method,
    pub name: SharedString,
    pub path: SharedString,
}

#[derive(Clone)]
pub struct Group {
    pub name: SharedString,
    pub endpoints: Vec<Endpoint>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum EnvKind {
    Dev,
    Prod,
}

impl EnvKind {
    pub const ALL: [EnvKind; 2] = [EnvKind::Dev, EnvKind::Prod];

    pub fn name(self) -> &'static str {
        match self {
            EnvKind::Dev => "Dev",
            EnvKind::Prod => "Prod",
        }
    }

    pub fn url(self) -> &'static str {
        match self {
            EnvKind::Dev => "https://dev.shop.local/api",
            EnvKind::Prod => "https://api.shop.com/v2",
        }
    }

    pub fn color(self, p: &Palette) -> Hsla {
        match self {
            EnvKind::Dev => p.online,
            EnvKind::Prod => p.danger,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum RailMode {
    Requests,
    Collections,
    Environments,
    History,
    Settings,
}

#[derive(Clone)]
pub struct RequestTab {
    pub method: Method,
    pub label: SharedString,
    /// Spec path this tab was opened from; identifies the endpoint in the tree.
    pub path: SharedString,
    /// What the user typed in the URL field.
    pub url: SharedString,
}

impl RequestTab {
    pub fn untitled() -> Self {
        Self {
            method: Method::Get,
            label: "Untitled".into(),
            path: "".into(),
            url: "".into(),
        }
    }
}

impl From<&Endpoint> for RequestTab {
    fn from(e: &Endpoint) -> Self {
        Self {
            method: e.method,
            label: e.name.clone(),
            path: e.path.clone(),
            url: format!("{{{{baseUrl}}}}{}", e.path).into(),
        }
    }
}

fn ep(method: Method, name: &'static str, path: &'static str) -> Endpoint {
    Endpoint {
        method,
        name: name.into(),
        path: path.into(),
    }
}

pub fn demo_groups() -> Vec<Group> {
    use Method::*;
    vec![
        Group {
            name: "orders".into(),
            endpoints: vec![
                ep(Get, "List orders", "/orders"),
                ep(Post, "Create order", "/orders"),
                ep(Get, "Get order", "/orders/{id}"),
                ep(Patch, "Update order", "/orders/{id}"),
                ep(Delete, "Delete order", "/orders/{id}"),
            ],
        },
        Group {
            name: "products".into(),
            endpoints: vec![
                ep(Get, "List products", "/products"),
                ep(Get, "Get product", "/products/{id}"),
            ],
        },
        Group {
            name: "customers".into(),
            endpoints: vec![
                ep(Get, "List customers", "/customers"),
                ep(Post, "Create customer", "/customers"),
            ],
        },
        Group {
            name: "auth".into(),
            endpoints: vec![
                ep(Post, "Login", "/auth/login"),
                ep(Post, "Refresh", "/auth/refresh"),
            ],
        },
    ]
}
