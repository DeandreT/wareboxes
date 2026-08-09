use leptos::{html, prelude::*};
use wareboxes_api_contract::v1::{
    CancelOutboundQaRequest, CompleteOutboundQaRequest, ConfigureOutboundQaPolicyRequest,
    OutboundQaCancellationReason, OutboundQaPolicyResponse, OutboundQaRequirement,
    OutboundQaSessionResponse, OutboundQaSessionStatus, OutboundQaSessionSummaryResponse,
    ShippingQueueEntryResponse, StartOutboundQaRequest, VerifyOutboundQaCartonRequest,
};

use crate::api;
use crate::components::{Icon, UiIcon};

#[derive(Clone, Debug, PartialEq, Eq)]
enum PendingQaCommand {
    Configure {
        request: ConfigureOutboundQaPolicyRequest,
        idempotency_key: String,
    },
    Start {
        packing_session_id: i64,
        request: StartOutboundQaRequest,
        idempotency_key: String,
    },
    Verify {
        session_id: i64,
        request: VerifyOutboundQaCartonRequest,
        idempotency_key: String,
    },
    Complete {
        session_id: i64,
        request: CompleteOutboundQaRequest,
        idempotency_key: String,
    },
    Cancel {
        session_id: i64,
        request: CancelOutboundQaRequest,
        idempotency_key: String,
    },
}

pub(super) fn outbound_qa_ready(entry: &ShippingQueueEntryResponse) -> bool {
    qa_state_ready(
        entry.outbound_qa_policy.as_ref(),
        entry.outbound_qa_session.as_ref(),
    )
}

fn qa_state_ready(
    policy: Option<&OutboundQaPolicyResponse>,
    session: Option<&OutboundQaSessionSummaryResponse>,
) -> bool {
    let Some(policy) = policy else {
        return true;
    };
    if policy.requirement == OutboundQaRequirement::NotRequired {
        return true;
    }
    session.is_some_and(|session| {
        session.policy_id == policy.policy_id
            && session.policy_revision == policy.revision
            && session.status == OutboundQaSessionStatus::Passed
            && session.progress.verified_carton_count == session.progress.expected_carton_count
    })
}

#[component]
pub(super) fn OutboundQaReadiness(
    entry: Signal<ShippingQueueEntryResponse>,
    can_configure: bool,
    pending: RwSignal<bool>,
    on_policy: Callback<OutboundQaPolicyResponse>,
    on_session: Callback<OutboundQaSessionResponse>,
    on_refresh: Callback<()>,
    on_unauthorized: Callback<()>,
) -> impl IntoView {
    let initial_entry = entry.get_untracked();
    let current_policy = RwSignal::new(initial_entry.outbound_qa_policy.clone());
    let current_session = RwSignal::new(initial_entry.outbound_qa_session.clone());
    let owner_id = initial_entry.inventory_owner_id;
    let facility_id = initial_entry.facility_id;
    let packing_session_id = initial_entry.packing_session_id;
    let order_revision = initial_entry.order_revision;
    let editor_open = RwSignal::new(false);
    let editor_requirement = RwSignal::new(
        current_policy
            .get_untracked()
            .as_ref()
            .map_or(OutboundQaRequirement::NotRequired, |policy| {
                policy.requirement
            }),
    );
    let scan_value = RwSignal::new(String::new());
    let cancel_open = RwSignal::new(false);
    let cancel_reason = RwSignal::new(OutboundQaCancellationReason::PackingCorrection);
    let cancel_note = RwSignal::new(String::new());
    let cancel_error = RwSignal::<Option<String>>::new(None);
    let retry = RwSignal::<Option<PendingQaCommand>>::new(None);
    let status = RwSignal::<Option<(bool, String)>>::new(None);
    let scan_input = NodeRef::<html::Input>::new();
    let cancel_reason_input = NodeRef::<html::Select>::new();
    let blocked = Signal::derive(move || pending.get() || retry.get().is_some());

    #[cfg(target_arch = "wasm32")]
    Effect::new(move |_| {
        let refreshed = entry.get();
        if !pending.get_untracked() && retry.get_untracked().is_none() {
            if !editor_open.get_untracked() {
                editor_requirement.set(
                    refreshed
                        .outbound_qa_policy
                        .as_ref()
                        .map_or(OutboundQaRequirement::NotRequired, |policy| {
                            policy.requirement
                        }),
                );
            }
            current_policy.set(refreshed.outbound_qa_policy);
            current_session.set(refreshed.outbound_qa_session);
        }
    });

    #[cfg(target_arch = "wasm32")]
    Effect::new(move |_| {
        if cancel_open.get() {
            if let Some(input) = cancel_reason_input.get() {
                let _ = input.focus();
            }
        }
    });

    #[cfg(target_arch = "wasm32")]
    Effect::new(move |_| {
        if qa_required(current_policy.get().as_ref())
            && qa_session_open(current_session.get().as_ref())
        {
            if let Some(input) = scan_input.get() {
                let _ = input.focus();
            }
        }
    });

    let dispatch = Callback::new(move |command: PendingQaCommand| {
        if pending.get_untracked() {
            return;
        }
        pending.set(true);
        status.set(Some((false, qa_pending_label(&command).to_owned())));
        let replay = command.clone();
        leptos::task::spawn_local(async move {
            let result = execute(command).await;
            let was_cancel = matches!(&replay, PendingQaCommand::Cancel { .. });
            pending.set(false);
            match result {
                Ok(QaCommandResult::Policy(policy)) => {
                    retry.set(None);
                    editor_open.set(false);
                    status.set(Some((false, "QA policy updated.".to_owned())));
                    current_policy.set(Some(policy.clone()));
                    current_session.set(None);
                    on_policy.run(policy);
                }
                Ok(QaCommandResult::Session(session)) => {
                    retry.set(None);
                    scan_value.set(String::new());
                    cancel_error.set(None);
                    cancel_open.set(false);
                    status.set(Some((
                        false,
                        match session.status {
                            OutboundQaSessionStatus::Passed => {
                                "Every carton passed outbound QA.".to_owned()
                            }
                            OutboundQaSessionStatus::Cancelled => {
                                "Outbound QA cancelled. Packing recovery is available.".to_owned()
                            }
                            OutboundQaSessionStatus::Open => format!(
                                "{} of {} cartons verified.",
                                session.progress.verified_carton_count,
                                session.progress.expected_carton_count,
                            ),
                        },
                    )));
                    current_session.set(
                        (session.status != OutboundQaSessionStatus::Cancelled)
                            .then(|| session_summary(&session)),
                    );
                    on_session.run(session);
                }
                Err(error) if error.unauthorized => on_unauthorized.run(()),
                Err(error) if error.ambiguous_outcome => {
                    if was_cancel {
                        cancel_open.set(false);
                    }
                    retry.set(Some(replay));
                    status.set(Some((
                        true,
                        "Outcome is unknown. Retry the exact saved command.".to_owned(),
                    )));
                }
                Err(error) => {
                    retry.set(None);
                    if was_cancel {
                        cancel_error.set(Some(error.message.clone()));
                    }
                    status.set(Some((true, error.message)));
                    on_refresh.run(());
                }
            }
        });
    });

    let configure = Callback::new(move |_| {
        dispatch.run(PendingQaCommand::Configure {
            request: ConfigureOutboundQaPolicyRequest {
                inventory_owner_id: owner_id,
                facility_id,
                requirement: editor_requirement.get_untracked(),
                expected_revision: current_policy.get_untracked().map(|policy| policy.revision),
            },
            idempotency_key: api::new_idempotency_key(),
        });
    });
    let start = Callback::new(move |_| {
        dispatch.run(PendingQaCommand::Start {
            packing_session_id,
            request: StartOutboundQaRequest {
                expected_order_revision: order_revision,
            },
            idempotency_key: api::new_idempotency_key(),
        });
    });
    let verify = Callback::new(move |_| {
        let Some(session) = current_session.get_untracked() else {
            return;
        };
        let carton_barcode = scan_value.get_untracked().trim().to_owned();
        if carton_barcode.is_empty() {
            status.set(Some((true, "Scan a carton barcode.".to_owned())));
            return;
        }
        dispatch.run(PendingQaCommand::Verify {
            session_id: session.session_id,
            request: VerifyOutboundQaCartonRequest {
                expected_revision: session.revision,
                carton_barcode,
            },
            idempotency_key: api::new_idempotency_key(),
        });
    });
    let complete = Callback::new(move |_| {
        if let Some(session) = current_session.get_untracked() {
            dispatch.run(PendingQaCommand::Complete {
                session_id: session.session_id,
                request: CompleteOutboundQaRequest {
                    expected_revision: session.revision,
                },
                idempotency_key: api::new_idempotency_key(),
            });
        }
    });
    let cancel = Callback::new(move |_| {
        let Some(session) = current_session.get_untracked() else {
            return;
        };
        let note = cancel_note.get_untracked().trim().to_owned();
        if cancel_reason.get_untracked() == OutboundQaCancellationReason::Other && note.is_empty() {
            cancel_error.set(Some(
                "A note is required when the cancellation reason is Other.".to_owned(),
            ));
            return;
        }
        cancel_error.set(None);
        dispatch.run(PendingQaCommand::Cancel {
            session_id: session.session_id,
            request: CancelOutboundQaRequest {
                expected_revision: session.revision,
                reason: cancel_reason.get_untracked(),
                note: (!note.is_empty()).then_some(note),
            },
            idempotency_key: api::new_idempotency_key(),
        });
    });
    let retry_exact = Callback::new(move |_| {
        if let Some(command) = retry.get_untracked() {
            retry.set(None);
            dispatch.run(command);
        }
    });

    view! {
        <div
            class="shipping-qa-cell"
            class:active=move || {
                qa_required(current_policy.get().as_ref())
                    && !qa_state_ready(
                        current_policy.get().as_ref(),
                        current_session.get().as_ref(),
                    )
            }
        >
            <span>"Outbound QA"</span>
            <strong>{move || qa_label(
                current_policy.get().as_ref(),
                current_session.get().as_ref(),
            )}</strong>
            <small>{move || qa_detail(
                current_policy.get().as_ref(),
                current_session.get().as_ref(),
            )}</small>
            <Show when=move || editor_open.get()>
                <div class="shipping-qa-policy-editor">
                    <label>
                        <span>"Requirement"</span>
                        <select
                            prop:value=move || requirement_wire(editor_requirement.get())
                            on:change=move |event| editor_requirement.set(
                                if event_target_value(&event) == "scan_every_carton" {
                                    OutboundQaRequirement::ScanEveryCarton
                                } else {
                                    OutboundQaRequirement::NotRequired
                                }
                            )
                            disabled=move || blocked.get()
                        >
                            <option value="not_required">"Not required"</option>
                            <option value="scan_every_carton">"Scan every carton"</option>
                        </select>
                    </label>
                    <button type="button" class="button primary-action" disabled=move || blocked.get() on:click=move |_| configure.run(())>"Apply"</button>
                    <button type="button" class="button secondary-action" disabled=move || blocked.get() on:click=move |_| editor_open.set(false)>"Cancel"</button>
                </div>
            </Show>
            <Show when=move || {
                qa_required(current_policy.get().as_ref())
                    && current_session.get().is_none()
                    && !editor_open.get()
            }>
                <button type="button" class="button primary-action" disabled=move || blocked.get() on:click=move |_| start.run(())>"Start carton QA"</button>
            </Show>
            <Show when=move || {
                qa_required(current_policy.get().as_ref())
                    && qa_session_open(current_session.get().as_ref())
            }>
                <form class="shipping-qa-scan" on:submit=move |event| { event.prevent_default(); verify.run(()); }>
                    <label>
                        <Icon icon=UiIcon::Scan/>
                        <input
                            node_ref=scan_input
                            autocomplete="off"
                            placeholder="Scan closed carton"
                            prop:value=move || scan_value.get()
                            on:input=move |event| scan_value.set(event_target_value(&event))
                            disabled=move || blocked.get()
                        />
                    </label>
                    <button type="submit" class="button secondary-action" disabled=move || blocked.get()>"Verify"</button>
                    <button
                        type="button"
                        class="button primary-action"
                        disabled=move || {
                            blocked.get()
                                || !qa_session_complete(current_session.get().as_ref())
                        }
                        on:click=move |_| complete.run(())
                    >"Pass QA"</button>
                </form>
            </Show>
            <Show when=move || can_configure && !editor_open.get()>
                <button type="button" class="button tertiary-action shipping-qa-configure" disabled=move || blocked.get() on:click=move |_| editor_open.set(true)>
                    {move || if current_policy.get().is_some() { "Change policy" } else { "Set policy" }}
                </button>
            </Show>
            <Show when=move || {
                can_configure
                    && current_session.get().is_some_and(|session| {
                        matches!(
                            session.status,
                            OutboundQaSessionStatus::Open | OutboundQaSessionStatus::Passed
                        )
                    })
            }>
                <button
                    type="button"
                    class="button secondary-action shipping-qa-cancel"
                    disabled=move || blocked.get()
                    on:click=move |_| {
                        cancel_reason.set(OutboundQaCancellationReason::PackingCorrection);
                        cancel_note.set(String::new());
                        cancel_error.set(None);
                        status.set(None);
                        cancel_open.set(true);
                    }
                >
                    <Icon icon=UiIcon::Reverse/>
                    "Cancel QA"
                </button>
            </Show>
            <Show when=move || status.get().is_some()>
                <p class:error=move || status.get().is_some_and(|status| status.0) role=move || status.get().is_some_and(|status| status.0).then_some("alert")>
                    {move || status.get().map(|status| status.1).unwrap_or_default()}
                </p>
            </Show>
            <Show when=move || retry.get().is_some()>
                <button type="button" class="button secondary-action" disabled=move || pending.get() on:click=move |_| retry_exact.run(())>"Retry exact command"</button>
            </Show>
            <Show when=move || cancel_open.get()>
                <div class="shipping-qa-dialog-backdrop">
                    <section
                        class="shipping-qa-dialog"
                        role="alertdialog"
                        aria-modal="true"
                        aria-labelledby="shipping-qa-cancel-title"
                    >
                        <header>
                            <div>
                                <span class="eyebrow">"Supervisor recovery"</span>
                                <h2 id="shipping-qa-cancel-title">"Cancel outbound QA"</h2>
                            </div>
                            <button
                                type="button"
                                class="icon-button"
                                aria-label="Close QA cancellation"
                                disabled=move || pending.get()
                                on:click=move |_| cancel_open.set(false)
                            ><Icon icon=UiIcon::Close/></button>
                        </header>
                        <p>
                            "Verified cartons remain in immutable history. Cancelling this attempt permits carton recovery or a fresh QA attempt before shipment creation."
                        </p>
                        <label>
                            <span>"Reason"</span>
                            <select
                                node_ref=cancel_reason_input
                                prop:value=move || cancellation_reason_wire(cancel_reason.get())
                                on:change=move |event| {
                                    cancel_reason.set(cancellation_reason_from_wire(
                                        &event_target_value(&event),
                                    ));
                                    cancel_error.set(None);
                                }
                                disabled=move || pending.get()
                            >
                                <option value="packing_correction">"Packing correction"</option>
                                <option value="quality_issue">"Quality issue"</option>
                                <option value="policy_error">"Policy error"</option>
                                <option value="operator_error">"Operator error"</option>
                                <option value="other">"Other"</option>
                            </select>
                        </label>
                        <label>
                            <span>"Note"</span>
                            <textarea
                                maxlength="500"
                                placeholder="Required for Other"
                                prop:value=move || cancel_note.get()
                                on:input=move |event| {
                                    cancel_note.set(event_target_value(&event));
                                    cancel_error.set(None);
                                }
                                disabled=move || pending.get()
                            ></textarea>
                        </label>
                        <Show when=move || cancel_error.get().is_some()>
                            <p class="error" role="alert">
                                {move || cancel_error.get().unwrap_or_default()}
                            </p>
                        </Show>
                        <footer>
                            <button
                                type="button"
                                class="button secondary-action"
                                disabled=move || pending.get()
                                on:click=move |_| cancel_open.set(false)
                            >"Keep QA active"</button>
                            <button
                                type="button"
                                class="button danger-action"
                                disabled=move || pending.get()
                                on:click=move |_| cancel.run(())
                            ><Icon icon=UiIcon::Reverse/>"Cancel QA attempt"</button>
                        </footer>
                    </section>
                </div>
            </Show>
        </div>
    }
}

fn session_summary(session: &OutboundQaSessionResponse) -> OutboundQaSessionSummaryResponse {
    OutboundQaSessionSummaryResponse {
        session_id: session.session_id,
        policy_id: session.policy_id,
        policy_revision: session.policy_revision,
        attempt: session.attempt,
        status: session.status,
        revision: session.revision,
        progress: session.progress,
        started_at: session.started_at.clone(),
        passed_at: session.passed_at.clone(),
        cancelled_at: session
            .cancellation
            .as_ref()
            .map(|cancellation| cancellation.cancelled_at.clone()),
    }
}

fn qa_required(policy: Option<&OutboundQaPolicyResponse>) -> bool {
    policy.is_some_and(|policy| policy.requirement == OutboundQaRequirement::ScanEveryCarton)
}

fn qa_session_open(session: Option<&OutboundQaSessionSummaryResponse>) -> bool {
    session.is_some_and(|session| session.status == OutboundQaSessionStatus::Open)
}

fn qa_session_complete(session: Option<&OutboundQaSessionSummaryResponse>) -> bool {
    session.is_some_and(|session| {
        session.progress.verified_carton_count == session.progress.expected_carton_count
    })
}

fn qa_label(
    policy: Option<&OutboundQaPolicyResponse>,
    session: Option<&OutboundQaSessionSummaryResponse>,
) -> &'static str {
    if qa_state_ready(policy, session) {
        if qa_required(policy) {
            "Passed"
        } else {
            "Not required"
        }
    } else if session.is_some() {
        "In progress"
    } else {
        "Required"
    }
}

fn qa_detail(
    policy: Option<&OutboundQaPolicyResponse>,
    session: Option<&OutboundQaSessionSummaryResponse>,
) -> String {
    session.map_or_else(
        || {
            policy.map_or("Default policy".to_owned(), |policy| {
                format!("Policy rev {}", policy.revision.get())
            })
        },
        |session| {
            format!(
                "{} / {} cartons",
                session.progress.verified_carton_count, session.progress.expected_carton_count,
            )
        },
    )
}

enum QaCommandResult {
    Policy(OutboundQaPolicyResponse),
    Session(OutboundQaSessionResponse),
}

async fn execute(command: PendingQaCommand) -> Result<QaCommandResult, api::ApiError> {
    match command {
        PendingQaCommand::Configure {
            request,
            idempotency_key,
        } => api::internal_post_idempotent(
            "/api/v1/outbound-qa-policies",
            &request,
            &idempotency_key,
        )
        .await
        .map(QaCommandResult::Policy),
        PendingQaCommand::Start {
            packing_session_id,
            request,
            idempotency_key,
        } => api::internal_post_idempotent(
            &format!("/api/v1/packing-sessions/{packing_session_id}/outbound-qa-sessions"),
            &request,
            &idempotency_key,
        )
        .await
        .map(QaCommandResult::Session),
        PendingQaCommand::Verify {
            session_id,
            request,
            idempotency_key,
        } => api::internal_post_idempotent(
            &format!("/api/v1/outbound-qa-sessions/{session_id}/carton-verifications"),
            &request,
            &idempotency_key,
        )
        .await
        .map(QaCommandResult::Session),
        PendingQaCommand::Complete {
            session_id,
            request,
            idempotency_key,
        } => api::internal_post_idempotent(
            &format!("/api/v1/outbound-qa-sessions/{session_id}/completions"),
            &request,
            &idempotency_key,
        )
        .await
        .map(QaCommandResult::Session),
        PendingQaCommand::Cancel {
            session_id,
            request,
            idempotency_key,
        } => api::internal_post_idempotent(
            &format!("/api/v1/outbound-qa-sessions/{session_id}/cancellations"),
            &request,
            &idempotency_key,
        )
        .await
        .map(QaCommandResult::Session),
    }
}

const fn requirement_wire(requirement: OutboundQaRequirement) -> &'static str {
    match requirement {
        OutboundQaRequirement::NotRequired => "not_required",
        OutboundQaRequirement::ScanEveryCarton => "scan_every_carton",
    }
}

fn qa_pending_label(command: &PendingQaCommand) -> &'static str {
    match command {
        PendingQaCommand::Configure { .. } => "Updating QA policy...",
        PendingQaCommand::Start { .. } => "Starting outbound QA...",
        PendingQaCommand::Verify { .. } => "Verifying carton...",
        PendingQaCommand::Complete { .. } => "Passing outbound QA...",
        PendingQaCommand::Cancel { .. } => "Cancelling outbound QA...",
    }
}

const fn cancellation_reason_wire(reason: OutboundQaCancellationReason) -> &'static str {
    match reason {
        OutboundQaCancellationReason::PackingCorrection => "packing_correction",
        OutboundQaCancellationReason::QualityIssue => "quality_issue",
        OutboundQaCancellationReason::PolicyError => "policy_error",
        OutboundQaCancellationReason::OperatorError => "operator_error",
        OutboundQaCancellationReason::Other => "other",
    }
}

fn cancellation_reason_from_wire(value: &str) -> OutboundQaCancellationReason {
    match value {
        "quality_issue" => OutboundQaCancellationReason::QualityIssue,
        "policy_error" => OutboundQaCancellationReason::PolicyError,
        "operator_error" => OutboundQaCancellationReason::OperatorError,
        "other" => OutboundQaCancellationReason::Other,
        _ => OutboundQaCancellationReason::PackingCorrection,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wareboxes_api_contract::v1::{
        OutboundQaProgressResponse, OutboundQaSessionSummaryResponse, Revision,
    };

    fn entry(
        requirement: Option<OutboundQaRequirement>,
        status: Option<OutboundQaSessionStatus>,
    ) -> ShippingQueueEntryResponse {
        ShippingQueueEntryResponse {
            order_id: 1,
            order_key: "SO-1".into(),
            order_revision: Revision::new(4).unwrap(),
            inventory_owner_id: 2,
            inventory_owner_name: "Owner".into(),
            facility_id: 3,
            facility_name: "DC".into(),
            facility_revision: Revision::new(1).unwrap(),
            packing_session_id: 4,
            rush: false,
            ship_by: None,
            origin_ready: true,
            destination_ready: true,
            outbound_qa_policy: requirement.map(|requirement| OutboundQaPolicyResponse {
                policy_id: 5,
                inventory_owner_id: 2,
                facility_id: 3,
                requirement,
                revision: Revision::new(1).unwrap(),
                configured_by: 6,
                configured_at: "2026-08-08T00:00:00Z".into(),
            }),
            outbound_qa_session: status.map(|status| OutboundQaSessionSummaryResponse {
                session_id: 7,
                policy_id: 5,
                policy_revision: Revision::new(1).unwrap(),
                attempt: 1,
                status,
                revision: Revision::new(4).unwrap(),
                progress: OutboundQaProgressResponse {
                    expected_carton_count: 2,
                    verified_carton_count: if status == OutboundQaSessionStatus::Passed {
                        2
                    } else {
                        1
                    },
                },
                started_at: "2026-08-08T00:00:00Z".into(),
                passed_at: (status == OutboundQaSessionStatus::Passed)
                    .then(|| "2026-08-08T00:01:00Z".into()),
                cancelled_at: (status == OutboundQaSessionStatus::Cancelled)
                    .then(|| "2026-08-08T00:01:00Z".into()),
            }),
            shipment: None,
        }
    }

    #[test]
    fn readiness_requires_the_current_policy_session_to_pass() {
        assert!(outbound_qa_ready(&entry(None, None)));
        assert!(outbound_qa_ready(&entry(
            Some(OutboundQaRequirement::NotRequired),
            None
        )));
        assert!(!outbound_qa_ready(&entry(
            Some(OutboundQaRequirement::ScanEveryCarton),
            Some(OutboundQaSessionStatus::Open)
        )));
        assert!(outbound_qa_ready(&entry(
            Some(OutboundQaRequirement::ScanEveryCarton),
            Some(OutboundQaSessionStatus::Passed)
        )));
        assert!(!outbound_qa_ready(&entry(
            Some(OutboundQaRequirement::ScanEveryCarton),
            Some(OutboundQaSessionStatus::Cancelled)
        )));
        assert_eq!(
            cancellation_reason_from_wire(cancellation_reason_wire(
                OutboundQaCancellationReason::PolicyError
            )),
            OutboundQaCancellationReason::PolicyError
        );
    }

    #[test]
    fn authoritative_session_response_advances_the_followup_revision() {
        let response = OutboundQaSessionResponse {
            session_id: 7,
            packing_session_id: 4,
            order_id: 1,
            inventory_owner_id: 2,
            facility_id: 3,
            policy_id: 5,
            policy_revision: Revision::new(1).unwrap(),
            attempt: 1,
            status: OutboundQaSessionStatus::Open,
            revision: Revision::new(3).unwrap(),
            progress: OutboundQaProgressResponse {
                expected_carton_count: 2,
                verified_carton_count: 2,
            },
            started_by: 6,
            started_at: "2026-08-08T00:00:00Z".into(),
            passed_by: None,
            passed_at: None,
            cancellation: None,
            verifications: Vec::new(),
        };

        let latest = session_summary(&response);
        assert_eq!(latest.revision, Revision::new(3).unwrap());
        assert!(qa_session_complete(Some(&latest)));
    }
}
