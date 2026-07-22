# Wareboxes Product Roadmap

This roadmap defines durable capability milestones and acceptance gates for building
Wareboxes into a multi-tenant facility execution platform. Milestones may overlap,
but a later capability must not bypass an unmet foundation gate.

## Development Policy

Until the production-readiness gate is declared, schema resets, fixture rebuilds,
and breaking internal contract changes are acceptable. Backward compatibility with
pre-production data or APIs is not a delivery constraint. Compatibility requirements
begin only when production data or published external contracts exist.

## Product Scope

Wareboxes will provide configurable execution for:

- Multi-tenant facility operators and multi-client 3PL facilities.
- Inbound planning, receiving, inspection, labeling, and putaway.
- Real-time inventory, containers, holds, traceability, counting, and adjustment.
- Order ingestion, reservation, allocation, replenishment, picking, packing,
  shipping, and loading.
- Returns, value-added services, yard, labor, billing, and automation workflows.
- External APIs, document exchange, event delivery, reporting, and support tooling.

An ERP or order-management system may remain the financial and commercial system of
record. Wareboxes owns facility execution and its auditable inventory consequences.

## Milestone 0: Safety and Platform Foundation

### Outcomes

- Establish tenant and inventory-owner isolation before expanding workflows.
- Establish production-grade command, audit, deployment, and recovery conventions.
- Define capacity envelopes and acceptance tests for supported facility profiles.

### Deliverables

- Tenant lifecycle, memberships, and selected-tenant request context.
- Facility and inventory-owner scopes for users and integration clients.
- `tenant_id` and `inventory_owner_id` propagation with scoped foreign keys and
  uniqueness constraints.
- PostgreSQL row-level security and fail-closed repository access.
- Versioned API conventions, stable error codes, request IDs, optimistic revisions,
  cursor pagination, and replay-safe idempotency records.
- Transactional outbox and inbox foundations.
- Restricted registration and CORS, protected credentials, login controls, short
  sessions, and production identity integration boundaries.
- Production container images, infrastructure definitions, managed secrets,
  telemetry, readiness, backups, and restore automation.
- Tenant-isolation, authorization, concurrency, migration, and load-test harnesses.
- Architecture decision records for tenancy, ownership, inventory, clients,
  integration, deployment cells, and configuration.

### Exit Gate

- Tenants with overlapping external identifiers cannot observe or mutate one
  another's data through APIs, repositories, workers, exports, events, or reports.
- Inventory cannot cross owner or facility boundaries without an explicit authorized
  transfer workflow.
- Retried commands return their original result without duplicating effects.
- Backup restoration and production-like deployment are repeatable.
- The agreed operational load envelope passes its latency and error budgets.

## Milestone 1: Inventory and Inbound Core

### Outcomes

- Make inventory auditable, replay-safe, owner-scoped, and continuously reconciled.
- Complete receiving through directed putaway for selected facility profiles.

### Deliverables

- Owner-scoped item master, UOM and pack conversion, item-facility policy, reason
  codes, zones, capacities, compatibility, and travel sequence.
- Immutable inventory transaction journal and signed entries.
- Transactional balance projections, reservations, allocations, and holds.
- Container/LPN hierarchy, lot and serial controls, expiration policy, status, and
  ownership disposition.
- Purchase order, ASN, transfer, return, and non-expected receipt contracts.
- Appointment, arrival, unload, expected/blind receipt, discrepancy, inspection,
  quarantine, labeling, cross-dock, and directed putaway.
- Scanner-first inbound and putaway workflows with typed exception handling.
- Count plans, blind counts, recounts, tolerances, approval, and adjustment posting.
- Inventory trace, recall, aging, and reconciliation views.

### Exit Gate

- A facility can receive, inspect, label, put away, move, hold, count, adjust, trace,
  and reconcile inventory without direct database intervention.
- Journal and balance projections reconcile continuously.
- Lot, serial, status, owner, UOM, location, and container invariants hold under
  concurrent operations and retries.

## Milestone 2: Outbound Fulfillment

### Outcomes

- Execute validated orders through allocation, picking, packing, shipping, and load
  confirmation.
- Support the pick methods required by selected facility profiles.

### Deliverables

- Complete order and line ingestion, validation, holds, changes, cancellation,
  priority, routing, and backorder policy.
- Soft reservation and concrete allocation with FIFO/FEFO, lot, serial, status,
  owner, facility, and location policy.
- Wave and waveless release, workload planning, pick work, and replenishment.
- Discrete, batch, cluster, case, pallet, zone, and cart workflows as required by
  supported profiles.
- Pick confirmation, short picks, substitutions, reversals, staging,
  consolidation, and supervisor exceptions.
- Cartonization, packing, scale and printer support, outbound QA, documents, labels,
  carrier adapters, manifesting, tracking, and shipment confirmation.
- Outbound load planning, staging-lane selection, trailer loading sequence, and
  departure confirmation.

### Exit Gate

- A validated order can complete through confirmed shipment with full stock and
  actor traceability.
- Cancellation, shortage, retry, reversal, and partial-shipment paths conserve
  inventory and leave recoverable work states.
- Shipment events and documents reconcile with inventory and external systems.

## Milestone 3: Configuration, Integration, and 3PL Operations

### Outcomes

- Onboard standard clients without source-code forks.
- Make integrations and client billing operable by authorized business users.

### Deliverables

- Versioned decision tables for receipt, putaway, allocation, replenishment, wave,
  pick, pack, count, document, and billing rules.
- Configuration inheritance, validation, simulation, approval, effective dating,
  promotion, audit, and rollback.
- Integration inbox/outbox, raw payload retention, mapping versions, quarantine,
  replay, delivery history, and operator console.
- Versioned REST APIs, signed webhooks, SFTP exchange, and prioritized business
  document standards and adapters.
- Customer portal for scoped inventory, order, shipment, document, and report access.
- Contracts, rate cards, billable events, storage snapshots, handling and accessorial
  charges, minimums, review, reconciliation, and financial export.
- Customer and vendor returns, inspection, disposition, relabeling, refurbishment,
  kitting, de-kitting, assembly, and selected value-added services.
- Yard appointments, gate workflows, trailers and containers, yard locations, spot
  moves, door assignments, detention, and loading/unloading status.

### Exit Gate

- A standard-profile inventory owner can be onboarded through configuration and
  mappings without a private product branch.
- Integration failures can be diagnosed, corrected, and replayed without database
  access.
- Billable events reconcile to facility operations and exported financial records.

## Milestone 4: Labor, Optimization, and Automation

### Outcomes

- Coordinate people, inventory, equipment, and automation against real-time demand.
- Improve throughput without making safe execution dependent on optimization.

### Deliverables

- Workforce attendance, direct and indirect time, standards, utilization,
  performance, skills, certifications, and equipment eligibility.
- Slotting and re-slotting recommendations with capacity and compatibility rules.
- Work prioritization, proximity, interleaving, congestion awareness, bottleneck
  visibility, and resource planning.
- Dynamic release, advanced replenishment, cross-docking, order streaming, and
  workload balancing.
- Vendor-neutral edge and adapter contracts for PLC, conveyor, robotics, sortation,
  printer, and scale systems.
- Automation health, command correlation, duplicate protection, recovery, and manual
  fallback workflows.

### Exit Gate

- High-volume and automated facilities meet their committed throughput and recovery
  envelopes.
- Optimization can be disabled while safe manual execution continues.
- Labor and equipment decisions are explainable, auditable, and reversible.

## Milestone 5: Fleet Scale and Operational Maturity

### Outcomes

- Operate a geographically distributed customer fleet with bounded blast radius.
- Provide predictable upgrades, recovery, support, security, and cost attribution.

### Deliverables

- Data-cell placement, tenant movement, dedicated-cell options, and regional data
  residency.
- Stateless API and worker scaling, workload-class isolation, connection pooling,
  read replicas, and measured table partitioning.
- Noisy-neighbor detection, per-tenant quotas, capacity management, and cost
  attribution.
- Tenant-ring and canary releases, feature controls, and automated schema deployment.
- Disaster-recovery exercises, security controls, compliance evidence, support
  playbooks, and customer-visible service history.
- CDC analytics, governed exports, ad hoc reporting, archival, retention, and purge.
- Tenant-safe support access with approval, expiration, reason capture, and immutable
  audit.

### Exit Gate

- Cell and regional recovery meet documented recovery objectives.
- The fleet sustains its committed workload with measured headroom and isolation.
- Tenant moves, upgrades, rollback, restore, and support-access procedures are
  routinely exercised.

## Cross-Cutting Definition of Done

A capability is complete only when it includes:

- Typed domain rules and explicit state transitions.
- Tenant, owner, facility, and permission enforcement.
- Atomic inventory, work, and audit effects where required.
- Idempotent retry and recovery behavior.
- Structured logs, metrics, traces, and operational alerts.
- Stable API and event contracts.
- Operator-facing exception diagnosis and recovery.
- Migration correctness and reproducible schema creation.
- Domain, concurrency, isolation, contract, workflow, and performance tests.
- User documentation and support procedures.
