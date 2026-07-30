use leptos::prelude::*;
use lucide_leptos::{CircleAlert, CircleCheck, Info, X};

#[cfg(target_arch = "wasm32")]
use std::time::Duration;

const MAX_VISIBLE_TOASTS: usize = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToastKind {
    Success,
    Error,
    Info,
}

impl ToastKind {
    const fn class_name(self) -> &'static str {
        match self {
            Self::Success => "toast toast-success",
            Self::Error => "toast toast-error",
            Self::Info => "toast toast-info",
        }
    }

    const fn role(self) -> &'static str {
        match self {
            Self::Error => "alert",
            Self::Success | Self::Info => "status",
        }
    }

    #[cfg(target_arch = "wasm32")]
    const fn duration(self) -> Duration {
        match self {
            Self::Success => Duration::from_millis(4_500),
            Self::Info => Duration::from_millis(6_000),
            Self::Error => Duration::from_millis(8_000),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Toast {
    id: u64,
    kind: ToastKind,
    message: String,
}

#[derive(Clone, Copy)]
pub struct ToastBus {
    toasts: RwSignal<Vec<Toast>>,
    next_id: RwSignal<u64>,
}

impl ToastBus {
    fn new() -> Self {
        Self {
            toasts: RwSignal::new(Vec::new()),
            next_id: RwSignal::new(1),
        }
    }

    pub fn success(self, message: impl Into<String>) {
        self.push(ToastKind::Success, message.into());
    }

    pub fn error(self, message: impl Into<String>) {
        self.push(ToastKind::Error, message.into());
    }

    pub fn info(self, message: impl Into<String>) {
        self.push(ToastKind::Info, message.into());
    }

    pub fn dismiss(self, id: u64) {
        let _ = self
            .toasts
            .try_update(|toasts| toasts.retain(|toast| toast.id != id));
    }

    fn push(self, kind: ToastKind, message: String) {
        let id = self.next_id.get_untracked();
        self.next_id.set(id.saturating_add(1));
        self.toasts.update(|toasts| {
            if toasts.len() >= MAX_VISIBLE_TOASTS {
                toasts.remove(0);
            }
            toasts.push(Toast { id, kind, message });
        });

        #[cfg(target_arch = "wasm32")]
        set_timeout(move || self.dismiss(id), kind.duration());
    }
}

pub fn use_toast_bus() -> ToastBus {
    expect_context::<ToastBus>()
}

#[component]
pub fn ToastProvider(children: Children) -> impl IntoView {
    let bus = ToastBus::new();
    provide_context(bus);

    children()
}

#[component]
pub fn ToastViewport() -> impl IntoView {
    let bus = use_toast_bus();
    view! {
        <div
            class="toast-viewport"
            class:hidden=move || bus.toasts.get().is_empty()
            aria-live="polite"
            aria-relevant="additions removals"
        >
            <For
                each=move || bus.toasts.get()
                key=|toast| toast.id
                children=move |toast| {
                    let id = toast.id;
                    let kind = toast.kind;
                    view! {
                        <div
                            class=kind.class_name()
                            role=kind.role()
                            aria-atomic="true"
                        >
                            <span class="toast-icon" aria-hidden="true">
                                {match kind {
                                    ToastKind::Success => {
                                        view! { <CircleCheck size=17/> }.into_any()
                                    }
                                    ToastKind::Error => {
                                        view! { <CircleAlert size=17/> }.into_any()
                                    }
                                    ToastKind::Info => {
                                        view! { <Info size=17/> }.into_any()
                                    }
                                }}
                            </span>
                            <p>{toast.message}</p>
                            <button
                                type="button"
                                class="toast-dismiss"
                                title="Dismiss notification"
                                aria-label="Dismiss notification"
                                on:click=move |_| bus.dismiss(id)
                            >
                                <X size=15/>
                            </button>
                        </div>
                    }
                }
            />
        </div>
    }
}
