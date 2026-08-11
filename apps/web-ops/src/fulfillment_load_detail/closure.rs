use leptos::{html, prelude::*};
use wareboxes_api_contract::v1::CloseInboundLoadRequest;
use wareboxes_core::models::{Load, Location};

use crate::api;
use crate::toast::use_toast_bus;

#[component]
pub(super) fn LoadClosureConfirmation(
    load: Load,
    locations: Vec<Location>,
    pending: RwSignal<bool>,
    error: RwSignal<Option<String>>,
    on_close: Callback<()>,
    on_refreshed: Callback<i64>,
    on_unauthorized: Callback<()>,
) -> impl IntoView {
    let load_scan = RwSignal::new(String::new());
    let location_scan = RwSignal::new(String::new());
    let retry_attempt = RwSignal::new(None::<(CloseInboundLoadRequest, String)>);
    let confirmation_ref = NodeRef::<html::Form>::new();
    let load_scan_ref = NodeRef::<html::Input>::new();
    let load_id = load.id;
    let reference = load
        .reference_number
        .clone()
        .unwrap_or_else(|| format!("Load #{load_id}"));
    let execution_barcode = load.execution_barcode.clone();
    let receiving_location = load
        .dock_door_location_id
        .and_then(|location_id| locations.iter().find(|location| location.id == location_id))
        .map(|location| {
            format!(
                "{} ({})",
                location.name.as_deref().unwrap_or("Receiving location"),
                location.barcode.as_deref().unwrap_or("no barcode")
            )
        })
        .unwrap_or_else(|| "No receiving location assigned".to_owned());
    let toasts = use_toast_bus();

    #[cfg(target_arch = "wasm32")]
    Effect::new(move |_| {
        if let Some(input) = load_scan_ref.get() {
            let _ = input.focus();
        }
        if let Some(form) = confirmation_ref.get() {
            form.scroll_into_view_with_bool(false);
        }
    });

    let submit = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        if pending.get_untracked() {
            return;
        }
        let (request, key) = if let Some(saved) = retry_attempt.get_untracked() {
            saved
        } else {
            let request = CloseInboundLoadRequest {
                load_scan: load_scan.get_untracked(),
                receiving_location_scan: location_scan.get_untracked(),
                closed_at: None,
            };
            let key = api::new_idempotency_key();
            retry_attempt.set(Some((request.clone(), key.clone())));
            (request, key)
        };
        pending.set(true);
        error.set(None);
        leptos::task::spawn_local(async move {
            match api::close_inbound_load(load_id, &request, &key).await {
                Ok(_) => {
                    retry_attempt.set(None);
                    pending.set(false);
                    on_close.run(());
                    toasts.success(format!("Inbound load #{load_id} closed."));
                    on_refreshed.run(load_id);
                }
                Err(api_error) if api_error.unauthorized => on_unauthorized.run(()),
                Err(api_error) => {
                    if !api_error.ambiguous_outcome {
                        retry_attempt.set(None);
                    }
                    toasts.error(api_error.message.clone());
                    error.set(Some(if api_error.ambiguous_outcome {
                        "Closure outcome is unknown. Retry to reconcile the exact saved scans."
                            .to_owned()
                    } else {
                        api_error.message
                    }));
                    pending.set(false);
                }
            }
        });
    };

    view! {
        <form
            node_ref=confirmation_ref
            class="confirmation-panel arrival-confirmation"
            role="alertdialog"
            aria-labelledby="close-inbound-load-title"
            on:submit=submit
        >
            <h3 id="close-inbound-load-title">"Verify trailer empty"</h3>
            <p>"Scan the received load and assigned dock after unloading is physically complete."</p>
            <div class="evidence-summary">
                <span><strong>"Load"</strong> {reference} " · " <span class="mono">{execution_barcode}</span></span>
                <span><strong>"Assigned dock"</strong> {receiving_location}</span>
            </div>
            <div class="form-grid two-column">
                <label>
                    <span>"Load scan"</span>
                    <input
                        node_ref=load_scan_ref
                        required
                        autocomplete="off"
                        prop:value=move || load_scan.get()
                        on:input=move |event| load_scan.set(event_target_value(&event))
                    />
                </label>
                <label>
                    <span>"Receiving dock scan"</span>
                    <input
                        required
                        autocomplete="off"
                        prop:value=move || location_scan.get()
                        on:input=move |event| location_scan.set(event_target_value(&event))
                    />
                </label>
            </div>
            <div class="form-actions">
                <button type="submit" class="button primary-action" disabled=move || pending.get()>
                    {move || if pending.get() { "Closing" } else { "Close inbound load" }}
                </button>
                <button type="button" class="button secondary-action" on:click=move |_| on_close.run(())>
                    "Go back"
                </button>
            </div>
        </form>
    }
}
