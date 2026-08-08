# Engineering Notes

- Keep invalid states unrepresentable. Prefer precise enums and workflow-specific request types over catch-all variants such as `generic`.
- Putaway and picking are RF scanner workflows, not task-manager task types.
- The task manager is for exceptional, scheduled, or generated work such as cycle counts and breaking master packs.
- Cancelling an order before physical execution releases its holds, reservations, and
  allocations without creating recovery work. Once outbound execution exists,
  cancellation recovery must be derived from immutable pick, stage, carton, and
  manifest records with exact stock provenance.
- Master packs and singles are separate items linked through explicit pack relationships; do not infer this from `packaging_unit`.
