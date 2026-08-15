# Wareboxes Architecture

Wareboxes is a modular monolith with separate deployable processes. Domain and
application behavior lives in reusable crates; applications are composition roots
that bind configuration, transports, persistence adapters, and process lifecycles.

## Workspace Boundaries

```text
apps/server                 API and server-rendered web process
apps/worker                 background delivery and scheduled-work process
apps/web-ops                desktop operations web application
apps/rf-android             scanner-first Android execution application

crates/domain               identifiers, value objects, invariants, state machines
crates/application          commands, queries, policies, and workflow contracts
crates/persistence-postgres PostgreSQL repositories, projections, RLS, migrations
crates/api-contract         versioned public request, response, and event schemas
crates/api                  Axum authentication, authorization, routes, and mapping
crates/worker               persistence-independent worker scheduling and delivery
crates/barcodes             barcode encoding primitives
```

Deployable applications remain under `apps/`. Reusable behavior belongs under
`crates/`. An application may depend on any crate needed to compose a process, but
the crates follow inward dependency direction:

```text
api / worker adapters / persistence-postgres
                  -> application
                  -> domain
```

`domain` has no dependency on transports or persistence. `application` may depend
on `domain`, but not on Axum, SQLx, Android APIs, or a concrete message publisher.
Persistence owns database records and mapping. API contracts own wire schemas.
Database rows, domain types, and public DTOs are mapped explicitly at their
boundaries.

## Runtime Shape

The server process handles authenticated commands and queries. PostgreSQL is the
transactional source of truth. Accepted state changes, inventory journal entries,
balance projection changes, work state, audit records, and outbox events commit in
the same transaction whenever they represent one business action.

The worker process claims committed outbox records using fenced leases and delivers
them through configured publishers. Delivery is at least once. Event keys,
idempotent consumers, retry classification, dead-letter state, and immutable
delivery attempts make retries observable and recoverable.

Long-running planning, reconciliation, export, and scheduled work uses worker
processes rather than request handlers. New network services require a measured
reason such as independent scaling, isolation, deployment cadence, or ownership.

## Data Isolation

Every operational record carries `tenant_id`. Owner-specific operations also carry
`inventory_owner_id`, and facility-specific operations carry `facility_id`.
Application authorization combines tenant membership, permission, site scope,
owner scope, and workflow attributes. PostgreSQL row-level security provides a
second fail-closed isolation boundary.

A tenant has one home data cell. A cell contains stateless API and worker replicas,
high-availability PostgreSQL with connection pooling, cache, object storage, and
observability. Dedicated cells are available when isolation, residency, or workload
requires them. Cell placement does not change domain or public API contracts.

## Inventory Consistency

Inventory uses an immutable transaction journal with signed entries and a current
balance projection updated in the same database transaction. Reservations promise
demand; allocations identify executable stock; holds restrict explicitly scoped
inventory. Internal movement entries conserve quantity across affected dimensions.

Retriable commands store tenant, operation, idempotency key, request hash, and the
original result. Identical retries return the original result. Reuse of a key with a
different request is rejected. Balance rows are locked in stable order, and
continuous reconciliation compares the journal, balances, allocations, shipments,
and external totals.

## Client Responsibilities

The web application is a dense desktop operations surface for planning,
administration, monitoring, and exception recovery. RF execution belongs in the
Rust Android application and is optimized for scanners, repeated actions,
interrupted connectivity, and device lifecycle recovery. Pack stations and edge
devices use dedicated workflows and narrow adapters for printers, scales, PLCs, and
material-handling equipment.

Offline permissions are defined per command. A disconnected client must not make
unbounded allocation or inventory decisions from stale state.

## Change Rules

- Add behavior to an existing module before creating a network service.
- Keep process configuration and lifecycle code in `apps/`.
- Keep transport-independent policies and commands in `application`.
- Keep SQL, database mapping, migrations, and RLS binding in
  `persistence-postgres`.
- Keep public versioned schemas in `api-contract` and map them at the API boundary.
- Require tenant context for every operational command and query.
- Treat replay safety, authorization, audit, recovery, and operator workflows as
  part of capability completion.
