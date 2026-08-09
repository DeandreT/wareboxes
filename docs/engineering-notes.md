# Engineering Notes

- Keep invalid states unrepresentable. Prefer precise enums and workflow-specific request types over catch-all variants such as `generic`.
- Putaway and picking are RF scanner workflows, not task-manager task types.
- The task manager is for exceptional, scheduled, or generated work such as cycle counts and breaking master packs.
- Cancelling an order before physical execution releases its holds, reservations, and
  allocations without creating recovery work. Once outbound execution exists,
  cancellation recovery must be derived from immutable pick, stage, carton, and
  manifest records with exact stock provenance.
- Master packs and singles are separate items linked through explicit pack relationships; do not infer this from `packaging_unit`.
- A short pick terminalizes the directed source allocation and records immutable
  physical evidence. Replacement inventory is selected through the typed shortage
  recovery workflow; generic order allocation cannot bypass active execution.
- An approved item substitution preserves the original shortage and discrepancy hold,
  records a versioned conversion policy, and creates a distinct substitute order line,
  reservation, allocation, and RF pick. Packing and shipping consume the derived
  physical demand while documents retain the accepted original-item quantity.
- Demand replenishment plans only loose, available stock from an explicit versioned
  reserve-source set. Scanner confirmation posts a conserved move into the pick face;
  it does not implicitly allocate that stock, so allocation remains an explicit,
  independently replay-safe decision.
