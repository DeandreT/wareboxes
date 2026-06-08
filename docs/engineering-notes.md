# Engineering Notes

- Keep invalid states unrepresentable. Prefer precise enums and workflow-specific request types over catch-all variants such as `generic`.
- Putaway and picking are RF scanner workflows, not task-manager task types.
- The task manager is for exceptional, scheduled, or generated work such as cycle counts, breaking master packs, and unpacking cancelled orders.
- Master packs and singles are separate items linked through explicit pack relationships; do not infer this from `packaging_unit`.
