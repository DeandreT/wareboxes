ALTER TABLE work_task_progress
    DROP CONSTRAINT work_task_progress_action_check,
    ADD CONSTRAINT work_task_progress_action_check CHECK (
        action IN (
            'started',
            'aborted',
            'expired',
            'scope_revoked',
            'completed',
            'cancelled',
            'progress',
            'unpacked',
            'missing',
            'damaged',
            'moved',
            'cycle_count_confirmed',
            'putaway_confirmed',
            'license_plate_putaway_confirmed',
            'putaway_heartbeat',
            'putaway_released',
            'inventory_relocation_confirmed',
            'inventory_relocation_heartbeat',
            'inventory_relocation_released',
            'cycle_count_heartbeat',
            'cycle_count_released'
        )
    );
