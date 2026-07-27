use super::*;

impl ExpectedReceivingReducer {
    #[must_use]
    pub const fn activity(&self) -> ReceivingActivity {
        match self.state {
            State::AwaitingLoad => ReceivingActivity::AwaitingLoad,
            State::ResolvingLoad { .. } => ReceivingActivity::ResolvingLoad,
            State::LoadResolutionFailed { .. } => ReceivingActivity::LoadResolutionFailed,
            State::Active(_) => ReceivingActivity::Active,
            State::ConfirmationPending { .. } => ReceivingActivity::ConfirmationPending,
            State::Refreshing { .. } => ReceivingActivity::Refreshing,
            State::RefreshFailed { .. } => ReceivingActivity::RefreshFailed,
            State::LoadComplete { .. } => ReceivingActivity::LoadComplete,
            State::ReconcileRequired { .. } => ReceivingActivity::ReconcileRequired,
        }
    }

    #[must_use]
    pub fn session(&self) -> Option<&ReceivingSession> {
        self.active().map(|active| &active.session)
    }

    #[must_use]
    pub fn selected_line(&self) -> Option<&ExpectedReceiptLine> {
        let active = self.active()?;
        active
            .draft
            .selected_line_id
            .and_then(|line_id| active.session.line(line_id))
    }

    #[must_use]
    pub fn confirmation_draft_view(&self) -> Option<ConfirmationDraftView<'_>> {
        let active = match &self.state {
            State::Active(active) | State::ConfirmationPending { active, .. } => active,
            _ => return None,
        };
        let draft = &active.draft;
        Some(ConfirmationDraftView {
            mode: draft.mode,
            selected_line_id: draft.selected_line_id,
            item_barcode: draft.item_barcode.as_ref(),
            dock_barcode: draft.dock_barcode.as_ref(),
            quantity: draft.quantity,
            container_capture: draft.container_capture,
            license_plate_barcode: draft.license_plate_barcode.as_ref(),
            exception_reason: draft.exception_reason,
            exception_note: draft.exception_note.as_ref().map(ExceptionNote::as_str),
        })
    }

    #[must_use]
    pub fn operator_error(&self) -> Option<&ReceivingOperatorError> {
        self.operator_error.as_ref()
    }

    #[must_use]
    pub const fn last_confirmation(&self) -> Option<ConfirmationSummary> {
        self.last_confirmation
    }

    #[must_use]
    pub const fn reconciliation_reason(&self) -> Option<ReconciliationReason> {
        match self.state {
            State::ReconcileRequired { reason } => Some(reason),
            _ => None,
        }
    }

    #[must_use]
    pub fn focus_target(&self) -> FocusTarget {
        match &self.state {
            State::AwaitingLoad | State::LoadResolutionFailed { .. } => {
                FocusTarget::Scanner(ScannerTarget::LoadBarcode)
            }
            State::ResolvingLoad { .. } => FocusTarget::Blocked(InteractionBlock::ResolvingLoad),
            State::ConfirmationPending { .. } => {
                FocusTarget::Blocked(InteractionBlock::ConfirmationPending)
            }
            State::Refreshing { .. } => FocusTarget::Blocked(InteractionBlock::Refreshing),
            State::RefreshFailed { .. } => FocusTarget::Blocked(InteractionBlock::RefreshFailed),
            State::LoadComplete { .. } => FocusTarget::Blocked(InteractionBlock::LoadComplete),
            State::ReconcileRequired { .. } => {
                FocusTarget::Blocked(InteractionBlock::ReconciliationRequired)
            }
            State::Active(active) => focus_for_draft(active),
        }
    }

    pub fn submit_scan(&mut self, value: &str) -> ReceivingTransition {
        match self.focus_target() {
            FocusTarget::Scanner(ScannerTarget::LoadBarcode) => self.scan_load(value),
            FocusTarget::Scanner(ScannerTarget::ItemBarcode) => self.scan_item(value),
            FocusTarget::Scanner(ScannerTarget::DockBarcode) => self.scan_dock(value),
            FocusTarget::Scanner(ScannerTarget::LicensePlateBarcode) => {
                self.scan_license_plate(value)
            }
            _ => ReceivingTransition::Blocked(ActionBlockReason::WorkflowBusy),
        }
    }

    pub fn scan_load(&mut self, value: &str) -> ReceivingTransition {
        if !matches!(
            self.state,
            State::AwaitingLoad | State::LoadResolutionFailed { .. } | State::LoadComplete { .. }
        ) {
            return ReceivingTransition::Blocked(ActionBlockReason::WorkflowBusy);
        }
        let barcode = match LoadBarcode::new(value) {
            Ok(barcode) => barcode,
            Err(_) => {
                self.operator_error = Some(ReceivingOperatorError::InvalidScan);
                return ReceivingTransition::Applied;
            }
        };
        let resolution_id = LoadResolutionId(self.next_id());
        self.state = State::ResolvingLoad {
            resolution_id,
            barcode: barcode.clone(),
        };
        self.operator_error = None;
        ReceivingTransition::Effect(ReceivingEffect::ResolveLoad {
            resolution_id,
            barcode,
        })
    }

    pub fn retry_load_resolution(&mut self) -> ReceivingTransition {
        let State::LoadResolutionFailed { barcode } = &self.state else {
            return ReceivingTransition::Ignored;
        };
        let barcode = barcode.clone();
        let resolution_id = LoadResolutionId(self.next_id());
        self.state = State::ResolvingLoad {
            resolution_id,
            barcode: barcode.clone(),
        };
        self.operator_error = None;
        ReceivingTransition::Effect(ReceivingEffect::ResolveLoad {
            resolution_id,
            barcode,
        })
    }

    pub fn load_resolved(
        &mut self,
        resolution_id: LoadResolutionId,
        session: ReceivingSession,
    ) -> ReceivingTransition {
        let State::ResolvingLoad {
            resolution_id: expected,
            barcode,
        } = &self.state
        else {
            return ReceivingTransition::Ignored;
        };
        if *expected != resolution_id {
            return ReceivingTransition::Ignored;
        }
        self.state = State::Active(ActiveSession {
            load_barcode: barcode.clone(),
            session,
            draft: ConfirmationDraft::default(),
        });
        self.operator_error = None;
        ReceivingTransition::Applied
    }

    pub fn load_resolution_failed(
        &mut self,
        resolution_id: LoadResolutionId,
        failure: LoadResolutionFailure,
    ) -> ReceivingTransition {
        let State::ResolvingLoad {
            resolution_id: expected,
            barcode,
        } = &self.state
        else {
            return ReceivingTransition::Ignored;
        };
        if *expected != resolution_id {
            return ReceivingTransition::Ignored;
        }
        let error = match failure {
            LoadResolutionFailure::NotFound => ReceivingOperatorError::LoadNotFound,
            LoadResolutionFailure::NotReady => ReceivingOperatorError::LoadNotReady,
            LoadResolutionFailure::Retryable => ReceivingOperatorError::ConnectionUnavailable,
            LoadResolutionFailure::InvalidResponse => {
                return self.reconcile(ReconciliationReason::InvalidServerState);
            }
        };
        self.operator_error = Some(error);
        self.state = State::LoadResolutionFailed {
            barcode: barcode.clone(),
        };
        ReceivingTransition::Applied
    }

    pub fn scan_item(&mut self, value: &str) -> ReceivingTransition {
        let barcode = match ItemBarcode::new(value) {
            Ok(barcode) => barcode,
            Err(_) => return self.set_operator_error(ReceivingOperatorError::InvalidScan),
        };
        let Some(active) = self.active_mut() else {
            return ReceivingTransition::Blocked(ActionBlockReason::NoActiveSession);
        };
        let matching = active
            .session
            .lines()
            .iter()
            .filter(|line| line.accepts(&barcode))
            .map(ExpectedReceiptLine::load_line_id)
            .collect::<Vec<_>>();
        match matching.as_slice() {
            [] => self.set_operator_error(ReceivingOperatorError::ItemNotExpected),
            [line_id] => {
                select_line(active, *line_id, Some(barcode));
                self.operator_error = None;
                ReceivingTransition::Applied
            }
            _ => {
                active.draft.selected_line_id = None;
                active.draft.item_barcode = Some(barcode);
                self.set_operator_error(ReceivingOperatorError::ItemMatchesMultipleLines {
                    line_ids: matching,
                })
            }
        }
    }

    pub fn select_line(&mut self, line_id: LoadLineId) -> ReceivingTransition {
        let Some(active) = self.active_mut() else {
            return ReceivingTransition::Blocked(ActionBlockReason::NoActiveSession);
        };
        let Some(line) = active.session.line(line_id) else {
            return self.set_operator_error(ReceivingOperatorError::LineNotOpen);
        };
        if active
            .draft
            .item_barcode
            .as_ref()
            .is_some_and(|barcode| !line.accepts(barcode))
        {
            return self.set_operator_error(ReceivingOperatorError::ItemDoesNotMatchLine);
        }
        let item_barcode = active.draft.item_barcode.clone();
        select_line(active, line_id, item_barcode);
        self.operator_error = None;
        ReceivingTransition::Applied
    }

    pub fn select_mode(&mut self, mode: ConfirmationMode) -> ReceivingTransition {
        let Some(active) = self.active_mut() else {
            return ReceivingTransition::Blocked(ActionBlockReason::NoActiveSession);
        };
        active.draft.mode = mode;
        active.draft.exception_reason = None;
        active.draft.exception_note = None;
        self.operator_error = None;
        ReceivingTransition::Applied
    }

    pub fn scan_dock(&mut self, value: &str) -> ReceivingTransition {
        let barcode = match DockBarcode::new(value) {
            Ok(barcode) => barcode,
            Err(_) => return self.set_operator_error(ReceivingOperatorError::InvalidScan),
        };
        let Some(active) = self.active_mut() else {
            return ReceivingTransition::Blocked(ActionBlockReason::NoActiveSession);
        };
        if active.draft.selected_line_id.is_none() {
            return ReceivingTransition::Blocked(ActionBlockReason::NoSelectedLine);
        }
        if active.draft.mode != ConfirmationMode::Received {
            return ReceivingTransition::Blocked(ActionBlockReason::WorkflowBusy);
        }
        if barcode != *active.session.dock().barcode() {
            return self.set_operator_error(ReceivingOperatorError::WrongReceivingDock);
        }
        active.draft.dock_barcode = Some(barcode);
        self.operator_error = None;
        ReceivingTransition::Applied
    }

    pub fn set_quantity(&mut self, quantity: i64) -> ReceivingTransition {
        let quantity = match PositiveQuantity::try_from(quantity) {
            Ok(quantity) => quantity,
            Err(_) => return self.set_operator_error(ReceivingOperatorError::InvalidQuantity),
        };
        let Some(active) = self.active_mut() else {
            return ReceivingTransition::Blocked(ActionBlockReason::NoActiveSession);
        };
        let Some(line) = active
            .draft
            .selected_line_id
            .and_then(|line_id| active.session.line(line_id))
        else {
            return ReceivingTransition::Blocked(ActionBlockReason::NoSelectedLine);
        };
        if quantity.get() > line.remaining().get() {
            return self.set_operator_error(ReceivingOperatorError::QuantityExceedsRemaining);
        }
        active.draft.quantity = Some(quantity);
        self.operator_error = None;
        ReceivingTransition::Applied
    }

    pub fn set_container_capture(&mut self, capture: ContainerCapture) -> ReceivingTransition {
        let Some(active) = self.active_mut() else {
            return ReceivingTransition::Blocked(ActionBlockReason::NoActiveSession);
        };
        if active.draft.mode != ConfirmationMode::Received {
            return ReceivingTransition::Blocked(ActionBlockReason::WorkflowBusy);
        }
        active.draft.container_capture = capture;
        if capture == ContainerCapture::Loose {
            active.draft.license_plate_barcode = None;
        }
        ReceivingTransition::Applied
    }

    pub fn scan_license_plate(&mut self, value: &str) -> ReceivingTransition {
        let barcode = match LicensePlateBarcode::new(value) {
            Ok(barcode) => barcode,
            Err(_) => return self.set_operator_error(ReceivingOperatorError::InvalidScan),
        };
        let Some(active) = self.active_mut() else {
            return ReceivingTransition::Blocked(ActionBlockReason::NoActiveSession);
        };
        if active.draft.mode != ConfirmationMode::Received
            || active.draft.container_capture != ContainerCapture::LicensePlate
        {
            return ReceivingTransition::Blocked(ActionBlockReason::WorkflowBusy);
        }
        active.draft.license_plate_barcode = Some(barcode);
        self.operator_error = None;
        ReceivingTransition::Applied
    }

    pub fn set_lot(&mut self, value: Option<&str>) -> ReceivingTransition {
        self.set_dimension(value, DimensionField::Lot)
    }

    pub fn set_serial(&mut self, value: Option<&str>) -> ReceivingTransition {
        self.set_dimension(value, DimensionField::Serial)
    }

    pub fn set_expiration(&mut self, value: Option<&str>) -> ReceivingTransition {
        let parsed = match value.map(Expiration::new).transpose() {
            Ok(parsed) => parsed,
            Err(_) => {
                return self.set_operator_error(ReceivingOperatorError::InvalidScan);
            }
        };
        let Some(active) = self.active_mut() else {
            return ReceivingTransition::Blocked(ActionBlockReason::NoActiveSession);
        };
        let expected = active
            .draft
            .selected_line_id
            .and_then(|line_id| active.session.line(line_id))
            .and_then(ExpectedReceiptLine::expiration);
        if expected.is_some() && expected != parsed.as_ref() {
            return self.set_operator_error(ReceivingOperatorError::DimensionDoesNotMatchExpected);
        }
        active.draft.expiration = parsed;
        self.operator_error = None;
        ReceivingTransition::Applied
    }

    pub fn set_exception_reason(&mut self, reason: ReceiptExceptionReason) -> ReceivingTransition {
        let Some(active) = self.active_mut() else {
            return ReceivingTransition::Blocked(ActionBlockReason::NoActiveSession);
        };
        active.draft.exception_reason = Some(reason);
        if reason != ReceiptExceptionReason::Other {
            active.draft.exception_note = None;
        }
        ReceivingTransition::Applied
    }

    pub fn set_exception_note(&mut self, value: Option<&str>) -> ReceivingTransition {
        let note = match value.map(ExceptionNote::new).transpose() {
            Ok(note) => note,
            Err(_) => {
                return self.set_operator_error(ReceivingOperatorError::InvalidScan);
            }
        };
        let Some(active) = self.active_mut() else {
            return ReceivingTransition::Blocked(ActionBlockReason::NoActiveSession);
        };
        active.draft.exception_note = note;
        ReceivingTransition::Applied
    }

    #[must_use]
    pub fn confirmation_guard(&self, access: CommandAccess) -> ActionGuard {
        if let CommandAccess::Blocked(reason) = access {
            return ActionGuard::Blocked(ActionBlockReason::Device(reason));
        }
        let State::Active(active) = &self.state else {
            return ActionGuard::Blocked(match self.state {
                State::LoadComplete { .. } => ActionBlockReason::LoadComplete,
                State::ReconcileRequired { .. } => ActionBlockReason::ReconciliationRequired,
                State::AwaitingLoad
                | State::ResolvingLoad { .. }
                | State::LoadResolutionFailed { .. } => ActionBlockReason::NoActiveSession,
                _ => ActionBlockReason::WorkflowBusy,
            });
        };
        guard_for_draft(active)
    }

    pub fn begin_confirmation(&mut self, access: CommandAccess) -> ReceivingTransition {
        if let ActionGuard::Blocked(reason) = self.confirmation_guard(access) {
            return ReceivingTransition::Blocked(reason);
        }
        let State::Active(active) = &self.state else {
            return ReceivingTransition::Blocked(ActionBlockReason::WorkflowBusy);
        };
        let Some(intent) = intent_for_draft(active) else {
            return self.reconcile(ReconciliationReason::CommandIntegrityFailure);
        };
        let active = active.clone();
        let confirmation_id = ConfirmationId(self.next_id());
        self.state = State::ConfirmationPending {
            active,
            confirmation_id,
            intent: intent.clone(),
        };
        self.operator_error = None;
        ReceivingTransition::Effect(ReceivingEffect::PersistConfirmation {
            confirmation_id,
            intent,
        })
    }

    /// Restores a persisted, unresolved confirmation without requiring a session refresh.
    ///
    /// The returned correlation ID must be used for the eventual durable command outcome.
    pub fn restore_pending_confirmation(
        &mut self,
        intent: ConfirmationIntent,
    ) -> Result<ConfirmationId, ReconciliationReason> {
        if !matches!(self.state, State::AwaitingLoad) || !intent.is_current_and_valid() {
            let reason = ReconciliationReason::CommandIntegrityFailure;
            self.reconcile(reason);
            return Err(reason);
        }
        let Some(active) = intent.restore_active() else {
            let reason = ReconciliationReason::CommandIntegrityFailure;
            self.reconcile(reason);
            return Err(reason);
        };
        let confirmation_id = ConfirmationId(self.next_id());
        self.state = State::ConfirmationPending {
            active,
            confirmation_id,
            intent,
        };
        self.operator_error = None;
        self.last_confirmation = None;
        Ok(confirmation_id)
    }

    pub fn confirmation_failed(
        &mut self,
        confirmation_id: ConfirmationId,
        failure: ConfirmationFailure,
    ) -> ReceivingTransition {
        let State::ConfirmationPending {
            active,
            confirmation_id: expected,
            ..
        } = &self.state
        else {
            return ReceivingTransition::Ignored;
        };
        if *expected != confirmation_id {
            return ReceivingTransition::Ignored;
        }
        match failure {
            ConfirmationFailure::Rejected => {
                self.state = State::Active(active.clone());
                self.operator_error = Some(ReceivingOperatorError::ConfirmationRejected);
                ReceivingTransition::Applied
            }
            ConfirmationFailure::CommandStillPending => ReceivingTransition::Applied,
            ConfirmationFailure::InvalidResponse => {
                self.reconcile(ReconciliationReason::CommandIntegrityFailure)
            }
        }
    }

    pub fn confirmation_succeeded(
        &mut self,
        confirmation_id: ConfirmationId,
        result: ConfirmationResult,
    ) -> ReceivingTransition {
        let State::ConfirmationPending {
            active,
            confirmation_id: expected,
            intent,
        } = &self.state
        else {
            return ReceivingTransition::Ignored;
        };
        if *expected != confirmation_id {
            return ReceivingTransition::Ignored;
        }
        let mut active = active.clone();
        let intent = intent.clone();
        if result.load_id != intent.load_id || result.load_line_id != intent.load_line_id {
            return self.reconcile(ReconciliationReason::ConfirmationIdentityMismatch);
        }
        if result.disposition != intent.command.disposition() {
            return self.reconcile(ReconciliationReason::ConfirmationDispositionMismatch);
        }
        if result.quantity != intent.command.quantity() {
            return self.reconcile(ReconciliationReason::ConfirmationQuantityMismatch);
        }
        let Some(line) = active.session.line_mut(result.load_line_id) else {
            return self.reconcile(ReconciliationReason::ConfirmationIdentityMismatch);
        };
        if let Err(reason) = validate_result_quantities(line, &result) {
            return self.reconcile(reason);
        }
        if let Err(reason) = line.apply_confirmation(&result) {
            return self.reconcile(reason);
        }

        let summary = ConfirmationSummary::from(result);
        self.last_confirmation = Some(summary);
        self.operator_error = None;
        if result.receive_completed {
            self.state = State::LoadComplete { summary };
            return ReceivingTransition::Applied;
        }
        let refresh_id = RefreshId(self.next_id());
        let load_id = active.session.load_id();
        self.state = State::Refreshing {
            active,
            refresh_id,
            summary,
        };
        ReceivingTransition::Effect(ReceivingEffect::RefreshSession {
            refresh_id,
            load_id,
        })
    }

    pub fn refresh_succeeded(
        &mut self,
        refresh_id: RefreshId,
        session: ReceivingSession,
    ) -> ReceivingTransition {
        let State::Refreshing {
            active,
            refresh_id: expected,
            summary,
        } = &self.state
        else {
            return ReceivingTransition::Ignored;
        };
        if *expected != refresh_id {
            return ReceivingTransition::Ignored;
        }
        if session.load_id() != active.session.load_id()
            || session.inventory_owner_id() != active.session.inventory_owner_id()
            || session.facility_id() != active.session.facility_id()
        {
            return self.reconcile(ReconciliationReason::RefreshAggregateMismatch);
        }
        if summary.remaining.get() > 0 && session.line(summary.load_line_id).is_none() {
            return self.reconcile(ReconciliationReason::RefreshAggregateMismatch);
        }
        for refreshed in session.lines() {
            if let Some(prior) = active.session.line(refreshed.load_line_id())
                && (refreshed.received().get() < prior.received().get()
                    || refreshed.rejected().get() < prior.rejected().get()
                    || refreshed.missing().get() < prior.missing().get())
            {
                return self.reconcile(ReconciliationReason::RefreshQuantityRegressed);
            }
        }
        self.state = State::Active(ActiveSession {
            load_barcode: active.load_barcode.clone(),
            session,
            draft: ConfirmationDraft::default(),
        });
        self.operator_error = None;
        ReceivingTransition::Applied
    }

    pub fn refresh_failed(
        &mut self,
        refresh_id: RefreshId,
        failure: RefreshFailure,
    ) -> ReceivingTransition {
        let State::Refreshing {
            active,
            refresh_id: expected,
            summary,
        } = &self.state
        else {
            return ReceivingTransition::Ignored;
        };
        if *expected != refresh_id {
            return ReceivingTransition::Ignored;
        }
        match failure {
            RefreshFailure::Retryable => {
                self.state = State::RefreshFailed {
                    active: active.clone(),
                    summary: *summary,
                };
                self.operator_error = Some(ReceivingOperatorError::ConnectionUnavailable);
                ReceivingTransition::Applied
            }
            RefreshFailure::NotFoundOrConflict | RefreshFailure::InvalidResponse => {
                self.reconcile(ReconciliationReason::InvalidServerState)
            }
        }
    }

    pub fn retry_refresh(&mut self) -> ReceivingTransition {
        let State::RefreshFailed {
            active, summary, ..
        } = &self.state
        else {
            return ReceivingTransition::Ignored;
        };
        let active = active.clone();
        let summary = *summary;
        let refresh_id = RefreshId(self.next_id());
        let load_id = active.session.load_id();
        self.state = State::Refreshing {
            active,
            refresh_id,
            summary,
        };
        self.operator_error = None;
        ReceivingTransition::Effect(ReceivingEffect::RefreshSession {
            refresh_id,
            load_id,
        })
    }

    pub fn require_reconciliation(&mut self, reason: ReconciliationReason) -> ReceivingTransition {
        self.reconcile(reason)
    }

    fn set_dimension(&mut self, value: Option<&str>, field: DimensionField) -> ReceivingTransition {
        let parsed = match value.map(StockDimension::new).transpose() {
            Ok(parsed) => parsed,
            Err(_) => {
                return self.set_operator_error(ReceivingOperatorError::InvalidScan);
            }
        };
        let Some(active) = self.active_mut() else {
            return ReceivingTransition::Blocked(ActionBlockReason::NoActiveSession);
        };
        let line = active
            .draft
            .selected_line_id
            .and_then(|line_id| active.session.line(line_id));
        let expected = match field {
            DimensionField::Lot => line.and_then(ExpectedReceiptLine::lot),
            DimensionField::Serial => line.and_then(ExpectedReceiptLine::serial),
        };
        if expected.is_some() && expected != parsed.as_ref() {
            return self.set_operator_error(ReceivingOperatorError::DimensionDoesNotMatchExpected);
        }
        match field {
            DimensionField::Lot => active.draft.lot = parsed,
            DimensionField::Serial => active.draft.serial = parsed,
        }
        self.operator_error = None;
        ReceivingTransition::Applied
    }

    fn active(&self) -> Option<&ActiveSession> {
        match &self.state {
            State::Active(active)
            | State::ConfirmationPending { active, .. }
            | State::Refreshing { active, .. }
            | State::RefreshFailed { active, .. } => Some(active),
            _ => None,
        }
    }

    fn active_mut(&mut self) -> Option<&mut ActiveSession> {
        match &mut self.state {
            State::Active(active) => Some(active),
            _ => None,
        }
    }

    fn next_id(&mut self) -> u64 {
        let id = self.next_correlation_id;
        self.next_correlation_id = self.next_correlation_id.saturating_add(1);
        id
    }

    fn set_operator_error(&mut self, error: ReceivingOperatorError) -> ReceivingTransition {
        self.operator_error = Some(error);
        ReceivingTransition::Applied
    }

    fn reconcile(&mut self, reason: ReconciliationReason) -> ReceivingTransition {
        self.state = State::ReconcileRequired { reason };
        self.operator_error = None;
        ReceivingTransition::ReconciliationRequired(reason)
    }
}
