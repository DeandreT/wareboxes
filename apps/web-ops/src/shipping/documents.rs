use leptos::prelude::*;
use wareboxes_api_contract::v1::{GeneratePackingSlipRequest, Revision, ShipmentDocumentResponse};
#[cfg(target_arch = "wasm32")]
use wareboxes_api_contract::v1::{GeneratePackingSlipResponse, ShipmentDocumentListResponse};

use crate::api;
use crate::components::{Icon, UiIcon};

#[derive(Clone, Debug, PartialEq, Eq)]
struct PendingGeneration {
    request: GeneratePackingSlipRequest,
    idempotency_key: String,
}

#[component]
pub(super) fn ShipmentDocumentsPanel(
    shipment_id: i64,
    shipment_revision: Revision,
    on_unauthorized: Callback<()>,
) -> impl IntoView {
    let documents = RwSignal::new(Vec::<ShipmentDocumentResponse>::new());
    let loading = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let retry = RwSignal::new(None::<PendingGeneration>);

    #[cfg(target_arch = "wasm32")]
    Effect::new(move |_| {
        refresh_documents(shipment_id, documents, loading, error, on_unauthorized)
    });

    let generate = Callback::new(move |_| {
        dispatch_generation(
            shipment_id,
            PendingGeneration {
                request: GeneratePackingSlipRequest {
                    expected_shipment_revision: shipment_revision,
                },
                idempotency_key: api::new_idempotency_key(),
            },
            documents,
            loading,
            error,
            retry,
            on_unauthorized,
        );
    });
    let retry_exact = Callback::new(move |_| {
        if let Some(command) = retry.get_untracked() {
            dispatch_generation(
                shipment_id,
                command,
                documents,
                loading,
                error,
                retry,
                on_unauthorized,
            );
        }
    });

    view! {
        <div class="shipping-documents" aria-label="Shipment documents">
            <div class="shipping-documents-heading">
                <div><h3>"Documents"</h3><span>{move || document_count_label(documents.get().len())}</span></div>
                <Show
                    when=move || !loading.get() && documents.get().is_empty()
                    fallback=move || loading.get().then(|| view! { <span class="status pending">"Working"</span> })
                >
                    <button
                        type="button"
                        class="button secondary-action"
                        disabled=move || loading.get() || retry.get().is_some()
                        on:click=move |_| generate.run(())
                    >
                        <Icon icon=UiIcon::Print/>
                        "Generate packing slip"
                    </button>
                </Show>
            </div>
            <Show when=move || error.get().is_some()>
                <div class="shipping-documents-error" role="alert">
                    <span>{move || error.get().unwrap_or_default()}</span>
                    <Show when=move || retry.get().is_some()>
                        <button type="button" class="button secondary-action" disabled=move || loading.get() on:click=move |_| retry_exact.run(())>
                            "Retry exact command"
                        </button>
                    </Show>
                </div>
            </Show>
            <For
                each=move || documents.get()
                key=|document| document.document_id
                children=move |document| {
                    let href = document_download_path(document.document_id);
                    let file_name = document.file_name.clone();
                    let generated = compact_generated_at(&document.generated_at);
                    let summary = format!(
                        "{} cartons · {} lines · {} units",
                        document.carton_count,
                        document.line_count,
                        document.demand.shipped_quantity,
                    );
                    view! {
                        <div class="shipping-document-row">
                            <span class="shipping-document-name"><strong>"Packing slip"</strong><small>{summary}</small></span>
                            <span class="shipping-document-meta">{generated}</span>
                            <a
                                class="icon-button"
                                href=href
                                download=file_name
                                title="Download packing slip"
                                aria-label="Download packing slip"
                            ><Icon icon=UiIcon::Download/></a>
                        </div>
                    }
                }
            />
        </div>
    }
}

#[cfg(target_arch = "wasm32")]
fn refresh_documents(
    shipment_id: i64,
    documents: RwSignal<Vec<ShipmentDocumentResponse>>,
    loading: RwSignal<bool>,
    error: RwSignal<Option<String>>,
    on_unauthorized: Callback<()>,
) {
    loading.set(true);
    leptos::task::spawn_local(async move {
        match api::internal_get::<ShipmentDocumentListResponse>(&document_list_path(shipment_id))
            .await
        {
            Ok(result) => {
                documents.set(result.documents);
                error.set(None);
            }
            Err(api_error) => {
                if api_error.unauthorized {
                    on_unauthorized.run(());
                }
                error.set(Some(api_error.message));
            }
        }
        loading.set(false);
    });
}

#[cfg(target_arch = "wasm32")]
#[allow(clippy::too_many_arguments)]
fn dispatch_generation(
    shipment_id: i64,
    command: PendingGeneration,
    documents: RwSignal<Vec<ShipmentDocumentResponse>>,
    loading: RwSignal<bool>,
    error: RwSignal<Option<String>>,
    retry: RwSignal<Option<PendingGeneration>>,
    on_unauthorized: Callback<()>,
) {
    if loading.get_untracked() {
        return;
    }
    loading.set(true);
    error.set(None);
    let retained = command.clone();
    leptos::task::spawn_local(async move {
        let result = api::internal_post_idempotent::<_, GeneratePackingSlipResponse>(
            &packing_slip_generation_path(shipment_id),
            &command.request,
            &command.idempotency_key,
        )
        .await;
        match result {
            Ok(result) => {
                documents.set(vec![result.document]);
                retry.set(None);
            }
            Err(api_error) => {
                if api_error.unauthorized {
                    on_unauthorized.run(());
                }
                retry.set(api_error.ambiguous_outcome.then_some(retained));
                error.set(Some(api_error.message));
                if !api_error.ambiguous_outcome && !api_error.unauthorized {
                    refresh_documents(shipment_id, documents, loading, error, on_unauthorized);
                    return;
                }
            }
        }
        loading.set(false);
    });
}

#[cfg(not(target_arch = "wasm32"))]
#[allow(clippy::too_many_arguments)]
fn dispatch_generation(
    _shipment_id: i64,
    _command: PendingGeneration,
    _documents: RwSignal<Vec<ShipmentDocumentResponse>>,
    _loading: RwSignal<bool>,
    _error: RwSignal<Option<String>>,
    _retry: RwSignal<Option<PendingGeneration>>,
    _on_unauthorized: Callback<()>,
) {
}

#[cfg(any(target_arch = "wasm32", test))]
fn document_list_path(shipment_id: i64) -> String {
    format!("/api/v1/shipments/{shipment_id}/documents")
}

#[cfg(any(target_arch = "wasm32", test))]
fn packing_slip_generation_path(shipment_id: i64) -> String {
    format!("/api/v1/shipments/{shipment_id}/documents/packing-slips")
}

fn document_download_path(document_id: i64) -> String {
    format!("/api/v1/shipment-documents/{document_id}/content")
}

fn document_count_label(count: usize) -> String {
    match count {
        0 => "Not generated".to_owned(),
        1 => "1 retained document".to_owned(),
        count => format!("{count} retained documents"),
    }
}

fn compact_generated_at(value: &str) -> String {
    value
        .split_once('T')
        .map_or(value, |(date, _)| date)
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn document_paths_and_labels_are_stable() {
        assert_eq!(document_list_path(7), "/api/v1/shipments/7/documents");
        assert_eq!(
            packing_slip_generation_path(7),
            "/api/v1/shipments/7/documents/packing-slips"
        );
        assert_eq!(
            document_download_path(9),
            "/api/v1/shipment-documents/9/content"
        );
        assert_eq!(document_count_label(0), "Not generated");
        assert_eq!(document_count_label(1), "1 retained document");
        assert_eq!(compact_generated_at("2026-08-08T22:00:00Z"), "2026-08-08");
    }
}
