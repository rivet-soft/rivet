//! Vertical scroll areas with a visible scrollbar (like the response body's).
//!
//! `scroll_y` keeps the element itself as the scroll container — its id and
//! test hooks stay as they are — and overlays gpui-kit's scrollbar driven by a
//! [`ScrollHandle`] that the owning view keeps in [`ScrollHandles`].

use std::cell::RefCell;
use std::collections::HashMap;

use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::*;

/// Scroll positions of a view's scroll areas, kept across renders.
#[derive(Default)]
pub struct ScrollHandles(RefCell<HashMap<SharedString, ScrollHandle>>);

impl ScrollHandles {
    /// The handle for scroll area `key` (created on first use).
    pub fn get(&self, key: impl Into<SharedString>) -> ScrollHandle {
        self.0.borrow_mut().entry(key.into()).or_default().clone()
    }
}

pub trait ScrollY: Sized {
    /// `overflow_y_scroll` plus a scrollbar on the right edge.
    fn scroll_y(self, handle: &ScrollHandle) -> Self;
}

impl ScrollY for Stateful<Div> {
    fn scroll_y(self, handle: &ScrollHandle) -> Self {
        self.overflow_y_scroll().track_scroll(handle).vertical_scrollbar(handle)
    }
}
