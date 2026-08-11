use leptos::prelude::*;
use lucide_leptos::{GripVertical, PanelLeftClose, PanelLeftOpen, PanelRightClose, PanelRightOpen};

const MIN_MASTER_WIDTH: i32 = 320;
const MAX_MASTER_WIDTH: i32 = 1_100;
const KEYBOARD_STEP: i32 = 24;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PaneMode {
    #[default]
    Both,
    MasterOnly,
    DetailOnly,
}

impl PaneMode {
    const fn storage_value(self) -> &'static str {
        match self {
            Self::Both => "both",
            Self::MasterOnly => "master",
            Self::DetailOnly => "detail",
        }
    }

    fn from_storage(value: &str) -> Option<Self> {
        match value {
            "both" => Some(Self::Both),
            "master" => Some(Self::MasterOnly),
            "detail" => Some(Self::DetailOnly),
            _ => None,
        }
    }

    pub const fn master_visible(self) -> bool {
        !matches!(self, Self::DetailOnly)
    }

    pub const fn detail_visible(self) -> bool {
        !matches!(self, Self::MasterOnly)
    }
}

#[derive(Clone, Copy)]
pub struct SplitPaneState {
    storage_key: &'static str,
    width: RwSignal<i32>,
    mode: RwSignal<PaneMode>,
    dragging: RwSignal<bool>,
    drag_origin: RwSignal<(i32, i32)>,
}

impl SplitPaneState {
    pub fn new(storage_key: &'static str, default_width: i32) -> Self {
        let state = Self {
            storage_key,
            width: RwSignal::new(clamp_width(default_width)),
            mode: RwSignal::new(PaneMode::Both),
            dragging: RwSignal::new(false),
            drag_origin: RwSignal::new((0, clamp_width(default_width))),
        };
        Effect::new(move || state.load());
        state
    }

    pub fn mode(self) -> PaneMode {
        self.mode.get()
    }

    pub fn style(self) -> String {
        format!("--split-master-width: {}px", self.width.get())
    }

    pub fn mode_attribute(self) -> &'static str {
        self.mode.get().storage_value()
    }

    pub fn toggle_master(self) {
        let next = match self.mode.get_untracked() {
            PaneMode::Both => PaneMode::DetailOnly,
            PaneMode::DetailOnly | PaneMode::MasterOnly => PaneMode::Both,
        };
        self.set_mode(next);
    }

    pub fn toggle_detail(self) {
        let next = match self.mode.get_untracked() {
            PaneMode::Both => PaneMode::MasterOnly,
            PaneMode::MasterOnly | PaneMode::DetailOnly => PaneMode::Both,
        };
        self.set_mode(next);
    }

    pub fn show_detail(self) {
        if self.mode.get_untracked() == PaneMode::MasterOnly {
            self.set_mode(PaneMode::Both);
        }
    }

    pub fn show_both(self) {
        self.set_mode(PaneMode::Both);
    }

    fn set_mode(self, mode: PaneMode) {
        self.mode.set(mode);
        browser::set(&mode_key(self.storage_key), mode.storage_value());
    }

    fn set_width(self, width: i32) {
        let width = clamp_width(width);
        self.width.set(width);
        browser::set(&width_key(self.storage_key), &width.to_string());
    }

    fn load(self) {
        if let Some(width) =
            browser::get(&width_key(self.storage_key)).and_then(|value| value.parse::<i32>().ok())
        {
            self.width.set(clamp_width(width));
        }
        if let Some(mode) = browser::get(&mode_key(self.storage_key))
            .as_deref()
            .and_then(PaneMode::from_storage)
        {
            self.mode.set(mode);
        }
    }

    fn begin_drag(self, event: leptos::ev::PointerEvent) {
        if self.mode.get_untracked() != PaneMode::Both {
            return;
        }
        self.drag_origin
            .set((event.client_x(), self.width.get_untracked()));
        self.dragging.set(true);
        capture_pointer(&event);
        event.prevent_default();
    }

    fn drag(self, event: leptos::ev::PointerEvent) {
        if !self.dragging.get_untracked() {
            return;
        }
        let (origin_x, origin_width) = self.drag_origin.get_untracked();
        self.width
            .set(clamp_width(origin_width + event.client_x() - origin_x));
    }

    fn end_drag(self) {
        if self.dragging.get_untracked() {
            self.dragging.set(false);
            self.set_width(self.width.get_untracked());
        }
    }

    fn key(self, event: leptos::ev::KeyboardEvent) {
        let next = match event.key().as_str() {
            "ArrowLeft" => Some(self.width.get_untracked() - KEYBOARD_STEP),
            "ArrowRight" => Some(self.width.get_untracked() + KEYBOARD_STEP),
            "Home" => Some(MIN_MASTER_WIDTH),
            "End" => Some(MAX_MASTER_WIDTH),
            _ => None,
        };
        if let Some(next) = next {
            event.prevent_default();
            self.set_width(next);
        }
    }
}

#[component]
pub fn PaneControls(
    layout: SplitPaneState,
    master_label: &'static str,
    detail_label: &'static str,
) -> impl IntoView {
    view! {
        <div class="pane-controls" aria-label="Workspace panes">
            <button
                type="button"
                class="icon-button"
                title=move || if layout.mode().master_visible() { format!("Hide {master_label}") } else { format!("Show {master_label}") }
                aria-label=move || if layout.mode().master_visible() { format!("Hide {master_label}") } else { format!("Show {master_label}") }
                disabled=move || layout.mode() == PaneMode::MasterOnly
                on:click=move |_| layout.toggle_master()
            >
                {move || if layout.mode().master_visible() { view! { <PanelLeftClose size=15/> }.into_any() } else { view! { <PanelLeftOpen size=15/> }.into_any() }}
            </button>
            <button
                type="button"
                class="icon-button"
                title=move || if layout.mode().detail_visible() { format!("Hide {detail_label}") } else { format!("Show {detail_label}") }
                aria-label=move || if layout.mode().detail_visible() { format!("Hide {detail_label}") } else { format!("Show {detail_label}") }
                disabled=move || layout.mode() == PaneMode::DetailOnly
                on:click=move |_| layout.toggle_detail()
            >
                {move || if layout.mode().detail_visible() { view! { <PanelRightClose size=15/> }.into_any() } else { view! { <PanelRightOpen size=15/> }.into_any() }}
            </button>
        </div>
    }
}

#[component]
pub fn SplitPaneHandle(layout: SplitPaneState) -> impl IntoView {
    view! {
        <div
            class="split-pane-handle"
            class:dragging=move || layout.dragging.get()
            class:collapsed=move || layout.mode() != PaneMode::Both
            role=move || if layout.mode() == PaneMode::Both { "separator" } else { "presentation" }
            aria-orientation="vertical"
            aria-label="Resize workspace panes"
            aria-valuemin=MIN_MASTER_WIDTH
            aria-valuemax=MAX_MASTER_WIDTH
            aria-valuenow=move || layout.width.get()
            tabindex=move || if layout.mode() == PaneMode::Both { "0" } else { "-1" }
            on:pointerdown=move |event| layout.begin_drag(event)
            on:pointermove=move |event| layout.drag(event)
            on:pointerup=move |_| layout.end_drag()
            on:pointercancel=move |_| layout.end_drag()
            on:keydown=move |event| layout.key(event)
        >
            {move || match layout.mode() {
                PaneMode::Both => view! { <GripVertical size=14/> }.into_any(),
                PaneMode::MasterOnly => view! {
                    <button
                        type="button"
                        class="split-pane-restore"
                        title="Restore detail pane"
                        aria-label="Restore detail pane"
                        on:click=move |event| {
                            event.stop_propagation();
                            layout.show_both();
                        }
                    >
                        <PanelRightOpen size=15/>
                    </button>
                }.into_any(),
                PaneMode::DetailOnly => view! {
                    <button
                        type="button"
                        class="split-pane-restore"
                        title="Restore list pane"
                        aria-label="Restore list pane"
                        on:click=move |event| {
                            event.stop_propagation();
                            layout.show_both();
                        }
                    >
                        <PanelLeftOpen size=15/>
                    </button>
                }.into_any(),
            }}
        </div>
    }
}

fn clamp_width(width: i32) -> i32 {
    width.clamp(MIN_MASTER_WIDTH, MAX_MASTER_WIDTH)
}

fn width_key(storage_key: &str) -> String {
    format!("wareboxes.layout.{storage_key}.master-width")
}

fn mode_key(storage_key: &str) -> String {
    format!("wareboxes.layout.{storage_key}.mode")
}

#[cfg(target_arch = "wasm32")]
fn capture_pointer(event: &leptos::ev::PointerEvent) {
    use wasm_bindgen::JsCast;

    if let Some(element) = event
        .current_target()
        .and_then(|target| target.dyn_into::<web_sys::Element>().ok())
    {
        drop(element.set_pointer_capture(event.pointer_id()));
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn capture_pointer(_event: &leptos::ev::PointerEvent) {}

#[cfg(target_arch = "wasm32")]
mod browser {
    use wasm_bindgen::prelude::*;

    #[wasm_bindgen]
    extern "C" {
        #[wasm_bindgen(js_namespace = localStorage, js_name = getItem, catch)]
        fn local_storage_get(key: &str) -> Result<JsValue, JsValue>;

        #[wasm_bindgen(js_namespace = localStorage, js_name = setItem, catch)]
        fn local_storage_set(key: &str, value: &str) -> Result<(), JsValue>;
    }

    pub fn get(key: &str) -> Option<String> {
        local_storage_get(key).ok()?.as_string()
    }

    pub fn set(key: &str, value: &str) {
        drop(local_storage_set(key, value));
    }
}

#[cfg(not(target_arch = "wasm32"))]
mod browser {
    pub fn get(_key: &str) -> Option<String> {
        None
    }

    pub fn set(_key: &str, _value: &str) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stored_modes_are_exact_and_never_hide_both_panes() {
        assert_eq!(PaneMode::from_storage("both"), Some(PaneMode::Both));
        assert_eq!(PaneMode::from_storage("master"), Some(PaneMode::MasterOnly));
        assert_eq!(PaneMode::from_storage("detail"), Some(PaneMode::DetailOnly));
        assert_eq!(PaneMode::from_storage("hidden"), None);
        assert!(PaneMode::MasterOnly.master_visible());
        assert!(PaneMode::DetailOnly.detail_visible());
    }

    #[test]
    fn pane_widths_are_bounded_for_desktop_workspaces() {
        assert_eq!(clamp_width(100), MIN_MASTER_WIDTH);
        assert_eq!(clamp_width(700), 700);
        assert_eq!(clamp_width(2_000), MAX_MASTER_WIDTH);
    }
}
