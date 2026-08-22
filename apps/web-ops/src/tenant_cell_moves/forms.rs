use leptos::prelude::*;
use wareboxes_api_contract::v1::{
    CancelTenantCellMoveRequest, CheckpointTenantCellMoveRequest, CompleteTenantCellMoveRequest,
    CutoverTenantCellMoveRequest, FreezeTenantCellMoveRequest, PlanTenantCellMoveRequest, Revision,
    RollbackTenantCellMoveRequest, StartTenantCellMoveCopyRequest, TenantCellMoveAction,
    TenantCellMoveCheckpointEvidence, TenantCellMoveCutoverVerificationEvidence,
    TenantCellMoveResponse, TenantCellMoveRollbackVerificationEvidence,
    TenantCellMoveValidationEvidence, ValidateTenantCellMoveRequest,
    VerifyTenantCellMoveCutoverRequest,
};

use super::{dispatch, Dialog, PendingCommand, Signals};
use crate::api;

#[derive(Clone, Copy)]
pub(super) struct Drafts {
    tenant_id: RwSignal<String>,
    target_cell_id: RwSignal<String>,
    placement_revision: RwSignal<String>,
    reason: RwSignal<String>,
    copy_reference: RwSignal<String>,
    source_lsn: RwSignal<String>,
    target_replay_lsn: RwSignal<String>,
    copied_row_count: RwSignal<String>,
    copied_bytes: RwSignal<String>,
    validation_json: RwSignal<String>,
    cutover_placement_revision: RwSignal<String>,
    verification_json: RwSignal<String>,
    rollback_verification_json: RwSignal<String>,
}

impl Drafts {
    pub(super) fn new() -> Self {
        Self {
            tenant_id: RwSignal::new(String::new()),
            target_cell_id: RwSignal::new(String::new()),
            placement_revision: RwSignal::new(String::new()),
            reason: RwSignal::new(String::new()),
            copy_reference: RwSignal::new(String::new()),
            source_lsn: RwSignal::new(String::new()),
            target_replay_lsn: RwSignal::new(String::new()),
            copied_row_count: RwSignal::new(String::new()),
            copied_bytes: RwSignal::new(String::new()),
            validation_json: RwSignal::new(String::new()),
            cutover_placement_revision: RwSignal::new(String::new()),
            verification_json: RwSignal::new(String::new()),
            rollback_verification_json: RwSignal::new(String::new()),
        }
    }

    pub(super) fn reset_plan(self) {
        self.tenant_id.set(String::new());
        self.target_cell_id.set(String::new());
        self.placement_revision.set(String::new());
        self.reason.set(String::new());
    }

    pub(super) fn reset_action(self, movement: &TenantCellMoveResponse) {
        self.reason.set(String::new());
        self.copy_reference
            .set(movement.copy_reference.clone().unwrap_or_default());
        self.source_lsn.set(
            movement
                .latest_checkpoint
                .as_ref()
                .map(|value| value.checkpoint.source_lsn.clone())
                .unwrap_or_default(),
        );
        self.target_replay_lsn.set(
            movement
                .latest_checkpoint
                .as_ref()
                .map(|value| value.checkpoint.target_replay_lsn.clone())
                .unwrap_or_default(),
        );
        self.copied_row_count.set(
            movement
                .latest_checkpoint
                .as_ref()
                .map(|value| value.checkpoint.copied_row_count.to_string())
                .unwrap_or_default(),
        );
        self.copied_bytes.set(
            movement
                .latest_checkpoint
                .as_ref()
                .map(|value| value.checkpoint.copied_bytes.to_string())
                .unwrap_or_default(),
        );
        self.cutover_placement_revision
            .set(movement.source_placement_revision.get().to_string());
        self.validation_json
            .set(validation_template(movement.latest_checkpoint.as_ref()));
        self.verification_json.set(verification_template(movement));
        self.rollback_verification_json
            .set(rollback_verification_template(movement));
    }
}

pub(super) fn dialog(signals: Signals, drafts: Drafts, value: Dialog) -> AnyView {
    let title = match &value {
        Dialog::Plan => "Plan tenant cell move",
        Dialog::Action(_, action) => super::display::action_label(*action),
    };
    view! {
        <div class="cell-move-dialog-backdrop" role="presentation">
            <section class="cell-move-dialog" role="dialog" aria-modal="true" aria-label=title>
                <header>
                    <div><p class="eyebrow">"Governed cell movement"</p><h2>{title}</h2></div>
                    <button class="text-button" type="button" disabled=move || signals.command_pending.get() on:click=move |_| signals.dialog.set(None)>"Close"</button>
                </header>
                {match value {
                    Dialog::Plan => plan_form(signals, drafts),
                    Dialog::Action(movement, action) => action_form(signals, drafts, *movement, action),
                }}
            </section>
        </div>
    }
    .into_any()
}

fn plan_form(signals: Signals, drafts: Drafts) -> AnyView {
    let submit = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        let tenant_id = match positive_i64(&drafts.tenant_id.get_untracked(), "tenant ID") {
            Ok(value) => value,
            Err(message) => return signals.command_error.set(Some(message)),
        };
        let target_data_cell_id =
            match positive_i64(&drafts.target_cell_id.get_untracked(), "target cell ID") {
                Ok(value) => value,
                Err(message) => return signals.command_error.set(Some(message)),
            };
        let expected_placement_revision = match revision(
            &drafts.placement_revision.get_untracked(),
            "placement revision",
        ) {
            Ok(value) => value,
            Err(message) => return signals.command_error.set(Some(message)),
        };
        let reason = drafts.reason.get_untracked().trim().to_owned();
        if reason.is_empty() {
            signals
                .command_error
                .set(Some("Enter an attributed reason for the move.".into()));
            return;
        }
        dispatch(
            signals,
            PendingCommand::Plan(
                tenant_id,
                PlanTenantCellMoveRequest {
                    target_data_cell_id,
                    expected_placement_revision,
                    reason,
                },
                api::new_idempotency_key(),
            ),
        );
    };
    view! {
        <form on:submit=submit>
            <section class="cell-move-warning"><strong>"Planning reserves target and rollback capacity."</strong><span>"Use the tenant placement revision you inspected. The service rejects changed placement, residency mismatch, and insufficient capacity."</span></section>
            <div class="cell-move-form-grid">
                <label><span>"Tenant ID"</span><input required type="number" min="1" prop:value=move || drafts.tenant_id.get() on:input=move |event| drafts.tenant_id.set(event_target_value(&event))/></label>
                <label><span>"Target data-cell ID"</span><input required type="number" min="1" prop:value=move || drafts.target_cell_id.get() on:input=move |event| drafts.target_cell_id.set(event_target_value(&event))/></label>
                <label><span>"Expected tenant placement revision"</span><input required type="number" min="1" prop:value=move || drafts.placement_revision.get() on:input=move |event| drafts.placement_revision.set(event_target_value(&event))/></label>
            </div>
            <label><span>"Attributed reason"</span><textarea required maxlength="500" rows="4" prop:value=move || drafts.reason.get() on:input=move |event| drafts.reason.set(event_target_value(&event))></textarea></label>
            {feedback(signals)}
            {footer(signals, "Plan move", false)}
        </form>
    }
    .into_any()
}

fn action_form(
    signals: Signals,
    drafts: Drafts,
    movement: TenantCellMoveResponse,
    action: TenantCellMoveAction,
) -> AnyView {
    let selected = movement.clone();
    let submit = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        let id = selected.tenant_cell_move_id;
        let expected_revision = selected.revision;
        let key = api::new_idempotency_key();
        let command = match build_action(drafts, id, expected_revision, action, key) {
            Ok(command) => command,
            Err(message) => return signals.command_error.set(Some(message)),
        };
        dispatch(signals, command);
    };
    let danger = matches!(
        action,
        TenantCellMoveAction::Freeze
            | TenantCellMoveAction::Cutover
            | TenantCellMoveAction::Rollback
            | TenantCellMoveAction::Cancel
    );
    view! {
        <form on:submit=submit>
            <dl class="cell-move-confirm">
                <div><dt>"Tenant"</dt><dd>{format!("{} ({})", movement.tenant.name, movement.tenant.tenant_id)}</dd></div>
                <div><dt>"Move / revision"</dt><dd>{format!("#{} / {}", movement.tenant_cell_move_id, movement.revision.get())}</dd></div>
                <div><dt>"Route"</dt><dd>{format!("{} → {}", movement.source_cell.key, movement.target_cell.key)}</dd></div>
                <div><dt>"Action"</dt><dd>{super::display::action_label(action)}</dd></div>
            </dl>
            {action_fields(drafts, &movement, action)}
            {feedback(signals)}
            {footer(signals, super::display::action_label(action), danger)}
        </form>
    }
    .into_any()
}

fn action_fields(
    drafts: Drafts,
    movement: &TenantCellMoveResponse,
    action: TenantCellMoveAction,
) -> AnyView {
    match action {
        TenantCellMoveAction::StartCopy => view! {
            <label><span>"Durable copy job reference"</span><input required maxlength="200" placeholder="copy-job-2026-08-21-0042" prop:value=move || drafts.copy_reference.get() on:input=move |event| drafts.copy_reference.set(event_target_value(&event))/></label>
        }.into_any(),
        TenantCellMoveAction::Checkpoint => view! {
            <div class="cell-move-form-grid">
                <label><span>"Source LSN"</span><input required placeholder="16/B374D848" prop:value=move || drafts.source_lsn.get() on:input=move |event| drafts.source_lsn.set(event_target_value(&event))/></label>
                <label><span>"Target replay LSN"</span><input required placeholder="16/B374D800" prop:value=move || drafts.target_replay_lsn.get() on:input=move |event| drafts.target_replay_lsn.set(event_target_value(&event))/></label>
                <label><span>"Copied rows"</span><input required type="number" min="0" prop:value=move || drafts.copied_row_count.get() on:input=move |event| drafts.copied_row_count.set(event_target_value(&event))/></label>
                <label><span>"Copied bytes"</span><input required type="number" min="0" prop:value=move || drafts.copied_bytes.get() on:input=move |event| drafts.copied_bytes.set(event_target_value(&event))/></label>
            </div>
        }.into_any(),
        TenantCellMoveAction::Freeze => view! {
            <section class="cell-move-warning danger"><strong>"Tenant writes will be fenced."</strong><span>"Confirm copy workers are healthy and the operator team is ready to validate promptly. Reads may continue while writes are frozen."</span></section>
        }.into_any(),
        TenantCellMoveAction::Validate => view! {
            <label><span>"Validation evidence JSON"</span><textarea class="evidence-json" required rows="18" spellcheck="false" prop:value=move || drafts.validation_json.get() on:input=move |event| drafts.validation_json.set(event_target_value(&event))></textarea></label>
            <p class="field-help">"Checksums and source/target counts must represent the frozen checkpoint. All verification flags must reflect completed checks."</p>
        }.into_any(),
        TenantCellMoveAction::Cutover => view! {
            <section class="cell-move-warning danger"><strong>"Cutover changes the tenant home cell."</strong><span>"The API atomically changes placement only when the source revision and fresh validation evidence still match."</span></section>
            <label><span>"Expected tenant placement revision"</span><input required type="number" min="1" prop:value=move || drafts.cutover_placement_revision.get() on:input=move |event| drafts.cutover_placement_revision.set(event_target_value(&event))/></label>
        }.into_any(),
        TenantCellMoveAction::VerifyCutover => view! {
            <label><span>"Post-cutover verification JSON"</span><textarea class="evidence-json" required rows="14" spellcheck="false" prop:value=move || drafts.verification_json.get() on:input=move |event| drafts.verification_json.set(event_target_value(&event))></textarea></label>
            <p class="field-help">{format!("Observed cell should be {} and placement revision should be the committed cutover revision.", movement.target_cell.data_cell_id)}</p>
        }.into_any(),
        TenantCellMoveAction::Complete => reason_field(drafts, "Completion reason"),
        TenantCellMoveAction::Rollback => view! {
            <section class="cell-move-warning danger"><strong>"Rollback changes the live tenant placement back to the source cell."</strong><span>"This is a second placement cutover, not an undo. Supply independent routing, source-read, fence, inventory, idempotency, and outbox proof. The expected rollback placement revision must be the cutover placement revision plus one."</span></section>
            <label><span>"Rollback safety verification JSON"</span><textarea class="evidence-json" required rows="14" spellcheck="false" prop:value=move || drafts.rollback_verification_json.get() on:input=move |event| drafts.rollback_verification_json.set(event_target_value(&event))></textarea></label>
            <p class="field-help">{format!("Observed cell must be the source cell (#{}) before the rollback transition is accepted.", movement.source_cell.data_cell_id)}</p>
            {reason_field(drafts, "Rollback reason")}
        }.into_any(),
        TenantCellMoveAction::Cancel => view! {
            <section class="cell-move-warning danger"><strong>"Cancellation releases the move reservations."</strong><span>"Cancel only before cutover; the evidence history remains available."</span></section>
            {reason_field(drafts, "Cancellation reason")}
        }.into_any(),
    }
}

fn reason_field(drafts: Drafts, label: &'static str) -> AnyView {
    view! {
        <label><span>{label}</span><textarea required maxlength="500" rows="4" prop:value=move || drafts.reason.get() on:input=move |event| drafts.reason.set(event_target_value(&event))></textarea></label>
    }
    .into_any()
}

fn build_action(
    drafts: Drafts,
    id: i64,
    expected_revision: Revision,
    action: TenantCellMoveAction,
    key: String,
) -> Result<PendingCommand, String> {
    match action {
        TenantCellMoveAction::StartCopy => {
            let copy_reference = drafts.copy_reference.get_untracked().trim().to_owned();
            if copy_reference.is_empty() {
                return Err("Enter the durable copy job reference.".into());
            }
            Ok(PendingCommand::StartCopy(
                id,
                StartTenantCellMoveCopyRequest {
                    expected_revision,
                    copy_reference,
                },
                key,
            ))
        }
        TenantCellMoveAction::Checkpoint => Ok(PendingCommand::Checkpoint(
            id,
            CheckpointTenantCellMoveRequest {
                expected_revision,
                checkpoint: TenantCellMoveCheckpointEvidence {
                    source_lsn: required(&drafts.source_lsn.get_untracked(), "source LSN")?,
                    target_replay_lsn: required(
                        &drafts.target_replay_lsn.get_untracked(),
                        "target replay LSN",
                    )?,
                    copied_row_count: nonnegative_i64(
                        &drafts.copied_row_count.get_untracked(),
                        "copied row count",
                    )?,
                    copied_bytes: nonnegative_i64(
                        &drafts.copied_bytes.get_untracked(),
                        "copied bytes",
                    )?,
                },
            },
            key,
        )),
        TenantCellMoveAction::Freeze => Ok(PendingCommand::Freeze(
            id,
            FreezeTenantCellMoveRequest { expected_revision },
            key,
        )),
        TenantCellMoveAction::Validate => {
            let validation = serde_json::from_str::<TenantCellMoveValidationEvidence>(
                &drafts.validation_json.get_untracked(),
            )
            .map_err(|error| format!("Validation evidence is not valid JSON: {error}"))?;
            Ok(PendingCommand::Validate(
                id,
                ValidateTenantCellMoveRequest {
                    expected_revision,
                    validation,
                },
                key,
            ))
        }
        TenantCellMoveAction::Cutover => Ok(PendingCommand::Cutover(
            id,
            CutoverTenantCellMoveRequest {
                expected_revision,
                expected_placement_revision: revision(
                    &drafts.cutover_placement_revision.get_untracked(),
                    "placement revision",
                )?,
            },
            key,
        )),
        TenantCellMoveAction::VerifyCutover => {
            let verification = serde_json::from_str::<TenantCellMoveCutoverVerificationEvidence>(
                &drafts.verification_json.get_untracked(),
            )
            .map_err(|error| format!("Cutover verification is not valid JSON: {error}"))?;
            Ok(PendingCommand::VerifyCutover(
                id,
                VerifyTenantCellMoveCutoverRequest {
                    expected_revision,
                    verification,
                },
                key,
            ))
        }
        TenantCellMoveAction::Complete => Ok(PendingCommand::Complete(
            id,
            CompleteTenantCellMoveRequest {
                expected_revision,
                reason: required(&drafts.reason.get_untracked(), "completion reason")?,
            },
            key,
        )),
        TenantCellMoveAction::Rollback => {
            let verification = serde_json::from_str::<TenantCellMoveRollbackVerificationEvidence>(
                &drafts.rollback_verification_json.get_untracked(),
            )
            .map_err(|error| format!("Rollback verification is not valid JSON: {error}"))?;
            Ok(PendingCommand::Rollback(
                id,
                RollbackTenantCellMoveRequest {
                    expected_revision,
                    verification,
                    reason: required(&drafts.reason.get_untracked(), "rollback reason")?,
                },
                key,
            ))
        }
        TenantCellMoveAction::Cancel => Ok(PendingCommand::Cancel(
            id,
            CancelTenantCellMoveRequest {
                expected_revision,
                reason: required(&drafts.reason.get_untracked(), "cancellation reason")?,
            },
            key,
        )),
    }
}

fn validation_template(
    checkpoint: Option<&wareboxes_api_contract::v1::TenantCellMoveCheckpointResponse>,
) -> String {
    let validation = TenantCellMoveValidationEvidence {
        tool_version: String::new(),
        source_lsn: checkpoint
            .map(|value| value.checkpoint.source_lsn.clone())
            .unwrap_or_default(),
        target_replay_lsn: checkpoint
            .map(|value| value.checkpoint.target_replay_lsn.clone())
            .unwrap_or_default(),
        source_row_count: checkpoint
            .map(|value| value.checkpoint.copied_row_count)
            .unwrap_or_default(),
        target_row_count: checkpoint
            .map(|value| value.checkpoint.copied_row_count)
            .unwrap_or_default(),
        source_data_checksum: String::new(),
        target_data_checksum: String::new(),
        source_schema_checksum: String::new(),
        target_schema_checksum: String::new(),
        source_object_manifest_checksum: String::new(),
        target_object_manifest_checksum: String::new(),
        inventory_reconciled: false,
        idempotency_verified: false,
        outbox_verified: false,
    };
    serde_json::to_string_pretty(&validation).unwrap_or_else(|_| "{}".into())
}

fn verification_template(movement: &TenantCellMoveResponse) -> String {
    let verification = TenantCellMoveCutoverVerificationEvidence {
        tool_version: String::new(),
        routing_reference: String::new(),
        observed_data_cell_id: movement.target_cell.data_cell_id,
        observed_placement_revision: movement
            .cutover_placement_revision
            .unwrap_or(movement.source_placement_revision),
        routing_verified: false,
        target_read_verified: false,
        write_fence_verified: false,
        inventory_reconciled: false,
        idempotency_verified: false,
        outbox_verified: false,
    };
    serde_json::to_string_pretty(&verification).unwrap_or_else(|_| "{}".into())
}

fn rollback_verification_template(movement: &TenantCellMoveResponse) -> String {
    let Some(expected_rollback_placement_revision) = movement
        .cutover_placement_revision
        .and_then(|revision| revision.get().checked_add(1))
        .and_then(|revision| Revision::new(revision).ok())
    else {
        return "{}".into();
    };
    let verification = TenantCellMoveRollbackVerificationEvidence {
        tool_version: String::new(),
        routing_reference: String::new(),
        observed_data_cell_id: movement.source_cell.data_cell_id,
        expected_rollback_placement_revision,
        routing_verified: false,
        source_read_verified: false,
        write_fence_verified: false,
        inventory_reconciled: false,
        idempotency_verified: false,
        outbox_verified: false,
    };
    serde_json::to_string_pretty(&verification).unwrap_or_else(|_| "{}".into())
}

fn footer(signals: Signals, submit_label: &'static str, danger: bool) -> AnyView {
    view! {
        <footer>
            <button class="button secondary-action" type="button" disabled=move || signals.command_pending.get() on:click=move |_| signals.dialog.set(None)>"Cancel"</button>
            <button class=if danger { "button danger-action" } else { "button primary-action" } type="submit" disabled=move || signals.command_pending.get()>{submit_label}</button>
        </footer>
    }
    .into_any()
}

fn feedback(signals: Signals) -> AnyView {
    view! {
        <>{move || signals.command_error.get().map(|message| view! { <p class="inline-command-error" role="alert">{message}</p> })}</>
    }
    .into_any()
}

fn required(value: &str, label: &str) -> Result<String, String> {
    let value = value.trim();
    if value.is_empty() {
        Err(format!("Enter {label}."))
    } else {
        Ok(value.to_owned())
    }
}

fn positive_i64(value: &str, label: &str) -> Result<i64, String> {
    value
        .trim()
        .parse::<i64>()
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| format!("Enter a positive {label}."))
}

fn nonnegative_i64(value: &str, label: &str) -> Result<i64, String> {
    value
        .trim()
        .parse::<i64>()
        .ok()
        .filter(|value| *value >= 0)
        .ok_or_else(|| format!("Enter a non-negative {label}."))
}

fn revision(value: &str, label: &str) -> Result<Revision, String> {
    let value = positive_i64(value, label)?;
    Revision::new(value).map_err(|_| format!("Enter a positive {label}."))
}
