//! Relay UI kit: small stateless (`RenderOnce`) building blocks styled after the
//! design tokens in [`crate::theme`]. Views own the state; components only render it
//! and report clicks back through callbacks.

mod badge;
mod body_mode_picker;
mod button;
mod field;
mod form_field_type_picker;
mod rail;
mod request_drag;
mod request_redirect_settings;
mod segmented;
mod splitter;
mod tab_actions;
mod tabs;
mod tree;

pub use badge::{Kbd, MethodBadge, Pill, StatusDot};
pub use body_mode_picker::BodyModePicker;
pub use button::{Button, ButtonVariant, IconButton};
pub use field::{
    SearchField, UrlField, split_vars, tokenize_vars, untokenized_vars, var_content, var_ranges,
    var_token_element,
};
pub use form_field_type_picker::FormFieldTypePicker;
pub use gpui_kit::assets::IconName;
pub use rail::RailItem;
pub use request_drag::{SavedRequestDrag, SavedRequestDragPreview};
pub use request_redirect_settings::RequestRedirectSettings;
pub use segmented::Segmented;
pub use splitter::{
    HorizontalPaneSplitter, PANE_SPLITTER_HEIGHT, PANE_SPLITTER_WIDTH, PaneSplitter,
};
pub use tab_actions::{TabAction, TabActionsMenu};
pub use tabs::{Tab, TabBar};
pub use tree::{SectionLabel, TreeRow};

use std::rc::Rc;

use gpui_kit::{App, ClickEvent, Hsla, Styled, Svg, Window, px, svg};

pub type ClickHandler = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

/// Text shown next to an action whose primary modifier differs by platform.
pub fn shortcut_hint(macos: &'static str, other: &'static str) -> &'static str {
    if cfg!(target_os = "macos") {
        macos
    } else {
        other
    }
}

/// Lucide icon tinted with `color`.
pub fn icon(name: IconName, size: f32, color: Hsla) -> Svg {
    svg()
        .path(name.path())
        .size(px(size))
        .flex_none()
        .text_color(color)
}

mod feedback;
pub use feedback::{EmptyState, ErrorState, LoadingState, ProgressBar, Spinner};

mod code;
mod table;
pub use code::{Annotation, CodeText, CodeView, TokenKind, tokenize_json_line};
pub use table::{KeyValueTable, KvRow};

mod overlay;
pub use overlay::{ConfirmPopover, EnvPopover, Placement, Popover, Tooltip};
