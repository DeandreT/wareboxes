# Vendor Return Operations

The **Vendor returns** workspace controls stock leaving a facility for a supplier.
Every return is scoped to one tenant, inventory owner, and facility and identifies
the exact inventory balances and quantities being returned.

## Create and release a return

1. Obtain the vendor's RGA or RMA and the item-level quality, recall, expiry, or
   overstock evidence.
2. Open **Vendor returns**, select the client and facility, enter a unique return
   number, vendor, optional vendor reference, and handling instructions.
3. Add each exact stock identity and quantity. Select a typed reason for every line;
   **Other** also requires explanatory evidence.
4. Create the draft and verify its item, location, lot, serial, license plate, and
   quantity in the detail pane.
5. Enter a staging note and select **Release and reserve**. Release atomically places
   quantity holds on every line. A stock conflict means another operation committed
   first; refresh and resolve it rather than editing a balance or hold directly.

## Ship or cancel

Use **Confirm shipment** only after the carrier receipt and physical departure are
verified. Shipment releases the return holds, posts negative entries for every line
to one immutable `return_to_vendor` inventory transaction, updates balances, records
the actor and transition, emits the committed lifecycle event through the outbox,
and captures a `return_unit` billable event when an effective client contract exists.

Draft and released returns may be cancelled with an attributed reason. Cancelling a
released return releases all of its holds without posting inventory entries. Shipped
and cancelled returns are terminal.

## Recovery and reconciliation

- If a command outcome is uncertain, use **Retry same command**. The console reuses
  the original idempotency key, so an accepted command returns its original result
  without reserving or shipping stock twice.
- A revision conflict means another actor changed the return. Refresh before choosing
  the next transition.
- The detail pane links the shipment inventory transaction and billable event and
  retains immutable lifecycle notes. Investigate missing evidence in Inventory
  control and Billing; never repair return, hold, journal, or billing rows directly.
- Support should correlate the vendor-return ID, request ID, inventory transaction
  ID, billable event ID, RGA/RMA, and outbox aggregate reference. Scope loss
  deliberately makes reads and idempotent replays appear not found.

Deferred database constraints reconcile state, holds, journal quantities, events,
and billing evidence at transaction commit. They also reject direct writes attempted
through the restricted runtime role outside the normal command path.
