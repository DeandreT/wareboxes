use leptos::prelude::*;
use leptos_router::components::A;
use lucide_leptos::{
    ArrowRightLeft, Boxes, Building2, ClipboardList, LayoutDashboard, LockKeyhole, LockKeyholeOpen,
    LogOut, PackageOpen, Plus, Printer, RefreshCw, ScanBarcode, Search, ShieldCheck, Trash2,
    TriangleAlert, Truck,
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum UiIcon {
    Access,
    Add,
    Alert,
    Building,
    Catalog,
    Clients,
    Counts,
    Disposition,
    Employees,
    Holds,
    Inventory,
    Loads,
    Orders,
    Packing,
    Overview,
    Permissions,
    Print,
    Refresh,
    Release,
    Remove,
    Roles,
    Scan,
    Search,
    Shipping,
    SignOut,
    Unlock,
    Users,
}

#[component]
pub fn Icon(icon: UiIcon) -> impl IntoView {
    view! {
        <span class="ui-icon" aria-hidden="true">
            {match icon {
                UiIcon::Access => view! { <ShieldCheck size=16/> }.into_any(),
                UiIcon::Add => view! { <Plus size=16/> }.into_any(),
                UiIcon::Alert => view! { <TriangleAlert size=16/> }.into_any(),
                UiIcon::Building => view! { <Building2 size=16/> }.into_any(),
                UiIcon::Catalog => view! { <Boxes size=16/> }.into_any(),
                UiIcon::Clients => view! { <Building2 size=16/> }.into_any(),
                UiIcon::Counts => view! { <ClipboardList size=16/> }.into_any(),
                UiIcon::Disposition => view! { <ArrowRightLeft size=16/> }.into_any(),
                UiIcon::Employees => view! { <ShieldCheck size=16/> }.into_any(),
                UiIcon::Holds => view! { <LockKeyhole size=16/> }.into_any(),
                UiIcon::Inventory => view! { <Boxes size=16/> }.into_any(),
                UiIcon::Loads => view! { <ClipboardList size=16/> }.into_any(),
                UiIcon::Orders => view! { <ClipboardList size=16/> }.into_any(),
                UiIcon::Packing => view! { <PackageOpen size=16/> }.into_any(),
                UiIcon::Overview => view! { <LayoutDashboard size=16/> }.into_any(),
                UiIcon::Permissions => view! { <LockKeyhole size=16/> }.into_any(),
                UiIcon::Print => view! { <Printer size=16/> }.into_any(),
                UiIcon::Refresh => view! { <RefreshCw size=16/> }.into_any(),
                UiIcon::Release => view! { <PackageOpen size=16/> }.into_any(),
                UiIcon::Remove => view! { <Trash2 size=16/> }.into_any(),
                UiIcon::Roles => view! { <ShieldCheck size=16/> }.into_any(),
                UiIcon::Scan => view! { <ScanBarcode size=16/> }.into_any(),
                UiIcon::Search => view! { <Search size=16/> }.into_any(),
                UiIcon::Shipping => view! { <Truck size=16/> }.into_any(),
                UiIcon::SignOut => view! { <LogOut size=16/> }.into_any(),
                UiIcon::Unlock => view! { <LockKeyholeOpen size=16/> }.into_any(),
                UiIcon::Users => view! { <ShieldCheck size=16/> }.into_any(),
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
