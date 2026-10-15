use std::rc::Rc;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::{ClickHandler, IconName, MethodBadge, icon};
use crate::model::Method;
use crate::theme::{ActivePalette, MONO};

const INDENT: f32 = 16.;

/// Row of the collections tree: either a folder (chevron + folder icon + count)
/// or a leaf request (method badge + name).
#[derive(IntoElement)]
pub struct TreeRow {
    id: ElementId,
    label: SharedString,
    depth: usize,
    kind: Kind,
    selected: bool,
    trailing_padding: f32,
    on_click: Option<ClickHandler>,
}

enum Kind {
    Folder { open: bool, count: Option<usize> },
    Request(Method),
}

impl TreeRow {
    pub fn folder(id: impl Into<ElementId>, label: impl Into<SharedString>, open: bool) -> Self {
        Self::with_kind(id, label, Kind::Folder { open, count: None })
    }

    pub fn request(
        id: impl Into<ElementId>,
        label: impl Into<SharedString>,
        method: Method,
    ) -> Self {
        Self::with_kind(id, label, Kind::Request(method))
    }

    fn with_kind(id: impl Into<ElementId>, label: impl Into<SharedString>, kind: Kind) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            depth: 0,
            kind,
            selected: false,
            trailing_padding: 10.,
            on_click: None,
        }
    }

    pub fn depth(mut self, depth: usize) -> Self {
        self.depth = depth;
        self
    }

    pub fn count(mut self, n: usize) -> Self {
        if let Kind::Folder { count, .. } = &mut self.kind {
            *count = Some(n);
        }
        self
    }

    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    pub fn trailing_padding(mut self, padding: f32) -> Self {
        self.trailing_padding = padding;
        self
    }

    pub fn on_click(mut self, f: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Rc::new(f));
        self
    }
}

impl RenderOnce for TreeRow {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let p = *cx.palette();
        let row = div()
            .id(self.id)
            .flex()
            .items_center()
            .gap(px(6.))
            .h(px(26.))
            .mx(px(4.))
            .pl(px(10. + INDENT * self.depth as f32))
            .pr(px(self.trailing_padding))
            .rounded(px(5.))
            .cursor_pointer()
            .when(self.selected, |s| s.bg(p.surface_hi))
            .hover(|s| s.bg(p.surface_hi))
            .when_some(self.on_click, |s, f| {
                s.on_click(move |e, w, cx| f(e, w, cx))
            });

        let label = div()
            .flex_1()
            .min_w_0()
            .overflow_hidden()
            .text_ellipsis()
            .whitespace_nowrap();

        match self.kind {
            Kind::Folder { open, count } => row
                .child(icon(
                    if open {
                        IconName::ChevronDown
                    } else {
                        IconName::ChevronRight
                    },
                    12.,
                    p.text_dim,
                ))
                .child(icon(IconName::Folder, 13., p.text_dim))
                .child(label.child(self.label))
                .when_some(count, |s, n| {
                    s.child(
                        div()
                            .font_family(MONO)
                            .text_size(px(10.))
                            .text_color(p.text_dim)
                            .child(n.to_string()),
                    )
                }),
            Kind::Request(m) => row
                .pl(px(28. + INDENT * self.depth as f32))
                .gap(px(8.))
                .child(MethodBadge::new(m).width(34.))
                .child(
                    label
                        .text_color(if self.selected { p.text } else { p.text_dim })
                        .child(self.label),
                ),
        }
        .test_support()
    }
}

/// Small dimmed heading inside a sidebar: `COLLECTIONS`, `Из спецификации`.
#[derive(IntoElement)]
pub struct SectionLabel {
    label: SharedString,
    caps: bool,
}

impl SectionLabel {
    pub fn new(label: impl Into<SharedString>) -> Self {
        Self {
            label: label.into(),
            caps: false,
        }
    }

    /// Bold uppercase panel title.
    pub fn caps(mut self) -> Self {
        self.caps = true;
        self
    }
}

impl RenderOnce for SectionLabel {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let p = cx.palette();
        div()
            .px(px(14.))
            .pt(px(10.))
            .pb(px(4.))
            .text_size(px(10.5))
            .text_color(p.text_dim)
            .when(self.caps, |s| s.font_weight(FontWeight::SEMIBOLD))
            .child(self.label)
    }
}
