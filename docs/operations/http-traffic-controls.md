# HTTP Traffic Controls

Wareboxes applies four process-local safety limits before application handlers:

- at most 256 non-service requests executing concurrently;
- at most 1,000 accepted non-service requests per second;
- at most 60 login attempts per minute;
- at most 30 seconds for one application request.

Liveness, readiness, and host-local metrics bypass these gates so an overloaded
process remains observable. Rejected rate-limit requests return `429` with the
stable error contract and `Retry-After`. Requests beyond concurrency capacity
return `503`; timed-out requests return `504`. Cancellation drops in-progress
database futures and PostgreSQL transactions roll back.

Configure the limits in `/etc/wareboxes/wareboxes.env` with
`MAX_IN_FLIGHT_REQUESTS`, `REQUEST_RATE_LIMIT_PER_SECOND`,
`LOGIN_RATE_LIMIT_PER_MINUTE`, and `REQUEST_TIMEOUT_SECONDS`. The defaults are
the accepted single-process baseline. When API replicas are added, enforce the
external client quota at the trusted ingress or a shared limiter as well; these
process-local limits remain the per-replica overload boundary.
