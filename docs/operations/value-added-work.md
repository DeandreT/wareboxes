# Value-Added Work Operations

The **Value-added work** workspace controls relabeling, refurbishment, kitting,
de-kitting, assembly, and other selected services. A work order is always scoped to
one tenant, inventory owner, and facility. It converts explicit input balance
identities into explicit output stock identities.

## Plan and release work

1. Create any required item batches, locations, and license plates in master data.
2. Open **Value-added work**, choose the client, facility, and typed workflow, and
   enter a unique work number.
3. Add input balances and quantities. Add output identities, dispositions, and
   quantities. The planner enforces each workflow's recipe shape. Relabeling and
   refurbishment also conserve total quantity.
4. Create the draft and verify the recipe in the detail pane.
5. Enter a physical staging note and select **Release and reserve**. Release places
   a quantity hold on every input atomically. If another operation consumed or held
   the stock first, refresh and resolve the conflict rather than changing a balance
   directly.

## Complete or cancel

Complete work only after scanning or otherwise verifying every input and finished
quantity. Completion releases the work holds, posts signed input and output entries
to one immutable inventory transaction, updates balances, captures a billable event
when an effective client contract exists, records the actor and transition, and
publishes the committed lifecycle event through the outbox.

Cancellation of draft or released work records an attributed reason. Released work
also releases all work holds without posting inventory entries. Completed and
cancelled work are terminal.

## Recovery and reconciliation

- If a command outcome is uncertain, use **Retry same command**. The console reuses
  the original idempotency key, so an accepted command returns its original result
  without applying inventory twice.
- A revision conflict means another actor changed the work. Refresh before deciding
  the next transition.
- A stock conflict means a source balance no longer has enough unreserved,
  unheld quantity. Inspect quantity holds and active work before replanning.
- The detail pane links the completion inventory transaction and billable event and
  shows immutable lifecycle evidence. Investigate any missing or mismatched evidence
  through Inventory control and Billing; never repair journal, hold, work, or billing
  rows directly.
- Support should correlate the work ID, request ID, inventory transaction ID,
  billable event ID, and outbox aggregate reference. Scope loss deliberately makes
  both reads and idempotent replays appear not found.

Database constraints reject recipes, holds, transitions, journal effects, or billing
evidence that do not reconcile at transaction commit, including writes attempted by
the restricted runtime role outside the normal repository path.
