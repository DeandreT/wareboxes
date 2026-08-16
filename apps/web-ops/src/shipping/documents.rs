use leptos::prelude::*;
#[cfg(test)]
use wareboxes_api_contract::v1::PRODUCT_DEFAULT_DOCUMENT_POLICY_HASH;
use wareboxes_api_contract::v1::{
    DocumentPolicyResponse, DocumentPolicySource, GenerateCartonLabelSetRequest,
    GeneratePackingSlipRequest, Revision, ShipmentDocumentResponse, ShipmentDocumentType,
    ShipmentStatus,
};
#[cfg(target_arch = "wasm32")]
use wareboxes_api_contract::v1::{
    GenerateCartonLabelSetResponse, GeneratePackingSlipResponse, ShipmentDocumentListResponse,
};

use crate::api;
use crate::components::{Icon, UiIcon};

#[derive(Clone, Debug, PartialEq, Eq)]
enum PendingGeneration {
    PackingSlip {
        request: GeneratePackingSlipRequest,
        idempotency_key: String,
    },
    CartonLabelSet {
        request: GenerateCartonLabelSetRequest,
        idempotency_key: String,
    },
}

#[component]
pub(super) fn ShipmentDocumentsPanel(
    shipment_id: i64,
    shipment_revision: Revision,
    shipment_status: ShipmentStatus,
    on_unauthorized: Callback<()>,
) -> impl IntoView {
    let documents = RwSignal::new(Vec::<ShipmentDocumentResponse>::new());
    let policy = RwSignal::new(None::<DocumentPolicyResponse>);
    let loading = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let retry = RwSignal::new(None::<PendingGeneration>);

    #[cfg(target_arch = "wasm32")]
    Effect::new(move |_| {
        refresh_documents(
            shipment_id,
            policy,
            documents,
            loading,
            error,
            on_unauthorized,
        )
    });

    let generate_packing_slip = Callback::new(move |_| {
        let Some(expected_policy) = policy.get_untracked().map(|current| current.expectation())
        else {
            return;
        };
        dispatch_generation(
            shipment_id,
            PendingGeneration::PackingSlip {
                request: GeneratePackingSlipRequest {
                    expected_shipment_revision: shipment_revision,
                    expected_policy,
                },
                idempotency_key: api::new_idempotency_key(),
            },
            policy,
            documents,
            loading,
            error,
            retry,
            on_unauthorized,
        );
    });
    let generate_carton_labels = Callback::new(move |_| {
        let Some(expected_policy) = policy.get_untracked().map(|current| current.expectation())
        else {
            return;
        };
        dispatch_generation(
            shipment_id,
            PendingGeneration::CartonLabelSet {
                request: GenerateCartonLabelSetRequest {
                    expected_shipment_revision: shipment_revision,
                    expected_policy,
                },
                idempotency_key: api::new_idempotency_key(),
            },
            policy,
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
                policy,
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
                <div><h3>"Documents"</h3><span>{move || format!("{} · {}",document_count_label(documents.get().len()),policy.get().as_ref().map_or_else(|| "policy loading".to_owned(),policy_label))}</span></div>
                <div class="shipping-document-actions">
                    <Show when=move || loading.get()><span class="status pending">"Working"</span></Show>
                    <Show when=move || policy.get().is_some_and(|current| current.generate_packing_slip) && !has_document(documents.get(), ShipmentDocumentType::PackingSlip)>
                        <button
                            type="button"
                            class="button secondary-action"
                            disabled=move || loading.get() || retry.get().is_some()
                            on:click=move |_| generate_packing_slip.run(())
                        >
                            <Icon icon=UiIcon::Print/>
                            "Packing slip"
                        </button>
                    </Show>
                    <Show when=move || policy.get().is_some_and(|current| current.generate_carton_label) && matches!(shipment_status, ShipmentStatus::Manifested) && !has_document(documents.get(), ShipmentDocumentType::CartonLabelSet)>
                        <button
                            type="button"
                            class="button secondary-action"
                            disabled=move || loading.get() || retry.get().is_some()
                            on:click=move |_| generate_carton_labels.run(())
                        >
                            <Icon icon=UiIcon::Print/>
                            "Carton labels"
                        </button>
                    </Show>
                </div>
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
                    let (name, summary) = document_display(&document);
                    let download_label = format!("Download {name}");
                    let download_title = download_label.clone();
                    view! {
                        <div class="shipping-document-row">
                            <span class="shipping-document-name"><strong>{name}</strong><small>{summary}</small></span>
                            <span class="shipping-document-meta">{format!("{} · {}",generated,policy_label(&document.policy))}</span>
                            <a
                                class="icon-button"
                                href=href
                                download=file_name
                                title=download_title
                                aria-label=download_label
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
    policy: RwSignal<Option<DocumentPolicyResponse>>,
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
                policy.set(Some(result.policy));
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
    policy: RwSignal<Option<DocumentPolicyResponse>>,
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
        let result = match &command {
            PendingGeneration::PackingSlip {
                request,
                idempotency_key,
            } => api::internal_post_idempotent::<_, GeneratePackingSlipResponse>(
                &packing_slip_generation_path(shipment_id),
                request,
                idempotency_key,
            )
            .await
            .map(|result| result.document),
            PendingGeneration::CartonLabelSet {
                request,
                idempotency_key,
            } => api::internal_post_idempotent::<_, GenerateCartonLabelSetResponse>(
                &carton_label_generation_path(shipment_id),
                request,
                idempotency_key,
            )
            .await
            .map(|result| result.document),
        };
        match result {
            Ok(document) => {
                documents.update(|current| {
                    current.retain(|entry| entry.document_type != document.document_type);
                    current.push(document);
                    current.sort_by_key(|entry| entry.document_id);
                });
                retry.set(None);
            }
            Err(api_error) => {
                if api_error.unauthorized {
                    on_unauthorized.run(());
                }
                retry.set(api_error.ambiguous_outcome.then_some(retained));
                error.set(Some(api_error.message));
                if !api_error.ambiguous_outcome && !api_error.unauthorized {
                    refresh_documents(
                        shipment_id,
                        policy,
                        documents,
                        loading,
                        error,
                        on_unauthorized,
                    );
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
    _policy: RwSignal<Option<DocumentPolicyResponse>>,
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

#[cfg(any(target_arch = "wasm32", test))]
fn carton_label_generation_path(shipment_id: i64) -> String {
    format!("/api/v1/shipments/{shipment_id}/documents/carton-label-sets")
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

fn has_document(
    documents: Vec<ShipmentDocumentResponse>,
    document_type: ShipmentDocumentType,
) -> bool {
    documents
        .iter()
        .any(|document| document.document_type == document_type)
}

fn document_display(document: &ShipmentDocumentResponse) -> (&'static str, String) {
    match document.document_type {
        ShipmentDocumentType::PackingSlip => (
            "Packing slip",
            format!(
                "{} cartons · {} lines · {} units",
                document.carton_count, document.line_count, document.demand.shipped_quantity,
            ),
        ),
        ShipmentDocumentType::CartonLabelSet => (
            "Carton labels",
            format!(
                "{} labels · {}{}",
                document.carton_count,
                document.carrier_code.as_deref().unwrap_or("Carrier"),
                document
                    .service_code
                    .as_deref()
                    .map_or_else(String::new, |service| format!(" / {service}")),
            ),
        ),
    }
}

#[cfg(test)]
fn product_default_policy() -> DocumentPolicyResponse {
    DocumentPolicyResponse {
        source: DocumentPolicySource::ProductDefault,
        configuration_id: None,
        configuration_revision: None,
        configuration_scope: None,
        generate_packing_slip: true,
        generate_carton_label: true,
        require_tracking_barcode: false,
        policy_hash: PRODUCT_DEFAULT_DOCUMENT_POLICY_HASH.to_owned(),
    }
}

fn policy_label(policy: &DocumentPolicyResponse) -> String {
    let source = match policy.source {
        DocumentPolicySource::ProductDefault => "product default".to_owned(),
        DocumentPolicySource::Configuration => format!(
            "configuration #{} r{}",
            policy.configuration_id.unwrap_or_default(),
            policy.configuration_revision.unwrap_or_default()
        ),
    };
    if policy.require_tracking_barcode {
        format!("{source} · tracking required")
    } else {
        source
    }
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
            carton_label_generation_path(7),
            "/api/v1/shipments/7/documents/carton-label-sets"
        );
        assert_eq!(
            document_download_path(9),
            "/api/v1/shipment-documents/9/content"
        );
        assert_eq!(document_count_label(0), "Not generated");
        assert_eq!(document_count_label(1), "1 retained document");
        assert_eq!(compact_generated_at("2026-08-08T22:00:00Z"), "2026-08-08");
        let default = product_default_policy();
        assert_eq!(policy_label(&default), "product default");
        assert_eq!(
            default.expectation().policy_hash,
            PRODUCT_DEFAULT_DOCUMENT_POLICY_HASH
        );
        let mut configured = default;
        configured.source = DocumentPolicySource::Configuration;
        configured.configuration_id = Some(17);
        configured.configuration_revision = Some(4);
        configured.require_tracking_barcode = true;
        assert_eq!(
            policy_label(&configured),
            "configuration #17 r4 · tracking required"
        );
    }
}
