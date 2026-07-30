use leptos::prelude::*;
use lucide_leptos::{ArrowDown, ArrowUp, ArrowUpDown};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SortDirection {
    Ascending,
    Descending,
}

impl SortDirection {
    pub fn reverse(self) -> Self {
        match self {
            Self::Ascending => Self::Descending,
            Self::Descending => Self::Ascending,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct SortSpec<K> {
    pub key: K,
    pub direction: SortDirection,
}

impl<K> SortSpec<K>
where
    K: Copy + PartialEq + Send + Sync + 'static,
{
    pub fn select(signal: RwSignal<Self>, key: K) {
        signal.update(|current| {
            if current.key == key {
                current.direction = current.direction.reverse();
            } else {
                current.key = key;
                current.direction = SortDirection::Ascending;
            }
        });
    }
}

#[component]
pub fn SortableHeader(
    label: &'static str,
    #[prop(into)] active: Signal<bool>,
    #[prop(into)] direction: Signal<SortDirection>,
    on_sort: Callback<()>,
    #[prop(optional)] numeric: bool,
    #[prop(optional)] column_class: &'static str,
) -> impl IntoView {
    let class = if numeric {
        format!("sortable-column numeric {column_class}")
    } else {
        format!("sortable-column {column_class}")
    };

    view! {
        <th
            scope="col"
            class=class
            aria-sort=move || {
                active.get().then_some(match direction.get() {
                    SortDirection::Ascending => "ascending",
                    SortDirection::Descending => "descending",
                })
            }
        >
            <button
                class="sort-button"
                class:active=move || active.get()
                type="button"
                on:click=move |_| on_sort.run(())
                aria-label=format!("Sort by {label}")
            >
                <span>{label}</span>
                <span class="sort-icon" aria-hidden="true">
                    {move || {
                        if !active.get() {
                            view! { <ArrowUpDown size=13/> }.into_any()
                        } else if direction.get() == SortDirection::Ascending {
                            view! { <ArrowUp size=13/> }.into_any()
                        } else {
                            view! { <ArrowDown size=13/> }.into_any()
                        }
                    }}
                </span>
            </button>
        </th>
    }
}
