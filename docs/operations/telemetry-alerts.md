# Telemetry and Alerts

The API emits structured JSON logs in production. HTTP spans include method, URI,
version, and the validated or generated `x-request-id`; command audit records,
integration attempts, and inventory transaction correlation IDs retain that request
identity where applicable. Keep logs in a restricted central sink and use request
IDs to correlate transport failures with durable workflow evidence.

Prometheus-format metrics are available only on the host-local `/metrics` endpoint.
Caddy intentionally returns 404 for public requests to that path. Configure one
scrape target per API process and load
`deploy/monitoring/wareboxes-alerts.yml` into the cell's Prometheus-compatible rule
evaluator. Preserve the `job="wareboxes-api"` label expected by the rules.

The rules are a minimum baseline. Receiver routing and paging credentials belong in
the deployment secret manager, not this repository. Also alert from the service
manager or log platform when any of these events is absent or failed:

- daily `event=backup_completed`;
- weekly `event=restore_drill_completed`;
- monthly `event=command_archive_completed`;
- worker dead-letter creation or repeated publisher failure;
- API or worker process restart loops.

## API target down

Confirm whether the process is stopped or merely unreachable from the scraper.
Check `systemctl status wareboxes.service`, recent structured logs, host capacity,
and `/health/live`. If liveness works, repair the private scrape route without
opening `/metrics` publicly.

## Readiness failure

Query `/health/ready` locally. Database failures and schema-contract failures are
logged separately. Check PostgreSQL availability, pool exhaustion, migration
version, runtime-role validation, disk capacity, and recent deploys. Remove the
replica from traffic until readiness is stable.

## High server error rate

Group structured error logs by request ID and route span, then correlate affected
commands with idempotency, audit, inbox, and outbox records. Do not retry mutating
requests without their original idempotency keys.

## Database pool saturation

Check query latency, locks, PostgreSQL connection count, and traffic-control
rejections before increasing pool capacity. More connections can worsen database
contention. Compare the event with the measured load envelope and scale replicas or
remove the blocking query only after identifying the constraint.

## Verification

After deployment, verify that liveness and readiness are independently scraped,
force one alert in a non-production environment, confirm receiver delivery, and
record the exercise. Repeat after monitoring topology, labels, or routing changes.
