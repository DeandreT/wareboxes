use leptos::prelude::*;
use leptos_router::components::A;
use lucide_leptos::{
    Boxes, Building2, ClipboardList, LayoutDashboard, LockKeyhole, LogOut, RefreshCw, Search,
    ShieldCheck, TriangleAlert,
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum UiIcon {
    Access,
    Alert,
    Building,
    Holds,
    Inventory,
    Orders,
    Overview,
    Refresh,
    Search,
    SignOut,
}

#[component]
pub fn Icon(icon: UiIcon) -> impl IntoView {
    view! {
        <span class="ui-icon" aria-hidden="true">
            {match icon {
                UiIcon::Access => view! { <ShieldCheck size=16/> }.into_any(),
                UiIcon::Alert => view! { <TriangleAlert size=16/> }.into_any(),
                UiIcon::Building => view! { <Building2 size=16/> }.into_any(),
                UiIcon::Holds => view! { <LockKeyhole size=16/> }.into_any(),
                UiIcon::Inventory => view! { <Boxes size=16/> }.into_any(),
                UiIcon::Orders => view! { <ClipboardList size=16/> }.into_any(),
                UiIcon::Overview => view! { <LayoutDashboard size=16/> }.into_any(),
                UiIcon::Refresh => view! { <RefreshCw size=16/> }.into_any(),
                UiIcon::Search => view! { <Search size=16/> }.into_any(),
                UiIcon::SignOut => view! { <LogOut size=16/> }.into_any(),
            }}
        </span>
    }
}

#[component]
pub fn NavItem(
    href: &'static str,
    label: &'static str,
    icon: UiIcon,
    active: bool,
) -> impl IntoView {
    view! {
        <A
            href
            attr:class=if active { "nav-link active" } else { "nav-link" }
            attr:aria-current=active.then_some("page")
        >
            <Icon icon/>
            <span>{label}</span>
        </A>
    }
}

#[component]
pub fn SearchField(
    label: String,
    placeholder: &'static str,
    value: RwSignal<String>,
) -> impl IntoView {
    view! {
        <label class="search-field">
            <span class="sr-only">{label}</span>
            <Icon icon=UiIcon::Search/>
            <input
                type="search"
                placeholder=placeholder
                prop:value=move || value.get()
                on:input=move |event| value.set(event_target_value(&event))
            />
        </label>
    }
}
