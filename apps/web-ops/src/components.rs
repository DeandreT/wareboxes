use leptos::prelude::*;
use leptos_router::components::A;
use lucide_leptos::{
    ArrowLeft, ArrowRightLeft, ArrowUpFromLine, Boxes, Building2, ClipboardList, Download, Factory,
    KeyRound, LayoutDashboard, ListChecks, LockKeyhole, LockKeyholeOpen, LogOut, PackageOpen,
    PackageSearch, Plus, Printer, RefreshCw, RotateCcw, ScanBarcode, Search, ShieldCheck,
    ShieldUser, Trash2, TriangleAlert, Truck, Warehouse, X,
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum UiIcon {
    Access,
    Add,
    Alert,
    Back,
    Building,
    Catalog,
    Clients,
    Close,
    Counts,
    CrossDock,
    Disposition,
    Download,
    Employees,
    Holds,
    Inventory,
    Loads,
    Orders,
    Orchestration,
    Packing,
    Overview,
    Permissions,
    Print,
    Putaway,
    Refresh,
    Replenishment,
    Reverse,
    Release,
    Remove,
    Roles,
    Scan,
    Search,
    Shipping,
    SignOut,
    Unlock,
    Users,
    Waves,
}

#[component]
pub fn Icon(icon: UiIcon) -> impl IntoView {
    view! {
        <span class="ui-icon" aria-hidden="true">
            {match icon {
                UiIcon::Access => view! { <ShieldCheck size=16/> }.into_any(),
                UiIcon::Add => view! { <Plus size=16/> }.into_any(),
                UiIcon::Alert => view! { <TriangleAlert size=16/> }.into_any(),
                UiIcon::Back => view! { <ArrowLeft size=16/> }.into_any(),
                UiIcon::Building => view! { <Factory size=16/> }.into_any(),
                UiIcon::Catalog => view! { <PackageSearch size=16/> }.into_any(),
                UiIcon::Clients => view! { <Building2 size=16/> }.into_any(),
                UiIcon::Close => view! { <X size=16/> }.into_any(),
                UiIcon::Counts => view! { <ListChecks size=16/> }.into_any(),
                UiIcon::CrossDock => view! { <ArrowRightLeft size=16/> }.into_any(),
                UiIcon::Disposition => view! { <ArrowRightLeft size=16/> }.into_any(),
                UiIcon::Download => view! { <Download size=16/> }.into_any(),
                UiIcon::Employees => view! { <ShieldUser size=16/> }.into_any(),
                UiIcon::Holds => view! { <LockKeyhole size=16/> }.into_any(),
                UiIcon::Inventory => view! { <Warehouse size=16/> }.into_any(),
                UiIcon::Loads => view! { <ClipboardList size=16/> }.into_any(),
                UiIcon::Orders => view! { <ClipboardList size=16/> }.into_any(),
                UiIcon::Orchestration => view! { <ClipboardList size=16/> }.into_any(),
                UiIcon::Packing => view! { <PackageOpen size=16/> }.into_any(),
                UiIcon::Overview => view! { <LayoutDashboard size=16/> }.into_any(),
                UiIcon::Permissions => view! { <LockKeyhole size=16/> }.into_any(),
                UiIcon::Print => view! { <Printer size=16/> }.into_any(),
                UiIcon::Putaway => view! { <Boxes size=16/> }.into_any(),
                UiIcon::Refresh => view! { <RefreshCw size=16/> }.into_any(),
                UiIcon::Replenishment => view! { <ArrowUpFromLine size=16/> }.into_any(),
                UiIcon::Reverse => view! { <RotateCcw size=16/> }.into_any(),
                UiIcon::Release => view! { <PackageOpen size=16/> }.into_any(),
                UiIcon::Remove => view! { <Trash2 size=16/> }.into_any(),
                UiIcon::Roles => view! { <KeyRound size=16/> }.into_any(),
                UiIcon::Scan => view! { <ScanBarcode size=16/> }.into_any(),
                UiIcon::Search => view! { <Search size=16/> }.into_any(),
                UiIcon::Shipping => view! { <Truck size=16/> }.into_any(),
                UiIcon::SignOut => view! { <LogOut size=16/> }.into_any(),
                UiIcon::Unlock => view! { <LockKeyholeOpen size=16/> }.into_any(),
                UiIcon::Users => view! { <ShieldUser size=16/> }.into_any(),
                UiIcon::Waves => view! { <ListChecks size=16/> }.into_any(),
            }}
        </span>
    }
}

#[component]
pub fn NavGroup(label: &'static str, active: bool, children: Children) -> impl IntoView {
    view! {
        <details class="nav-section" class:contains-active=active open=active>
            <summary>
                <span>{label}</span>
                <span class="nav-section-chevron" aria-hidden="true"></span>
            </summary>
            <div class="nav-section-items">{children()}</div>
        </details>
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
