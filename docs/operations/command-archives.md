# Durable Command Partitioning and Archives

`command_idempotency_records` is the permanent replay index and result store for
accepted commands. It is hash-partitioned into 16 partitions by `tenant_id`.
PostgreSQL enforces the unique `(tenant_id, operation, idempotency_key)` identity
on the partitioned parent, so two partitions cannot admit the same tenant-scoped
command identity. The runtime role has privileges on the parent only and cannot
query or mutate a child partition directly.

Every durable command has a non-null `actor_user_id` protected by a tenant-scoped
foreign key. Records are immutable. The archive workflow does not update or delete
them, so exact online replay, request-hash conflict detection, result schema
versioning, and actor attribution continue to use the canonical row.

## Archive workflow

Provisioned hosts run `wareboxes-command-archive.timer` monthly. The job selects
records older than 90 days in deterministic `(created, tenant_id, id)` order and
streams a versioned JSON Lines representation directly into the encrypted restic
repository. It does not write an unencrypted intermediate file. Each snapshot is
cumulative through its cutoff, allowing restic to deduplicate unchanged content.

The job reads the encrypted object back, verifies its row count and SHA-256 digest,
and only then applies the default retention of 24 monthly snapshots plus the latest
snapshot. It emits `event=command_archive_completed` with the cutoff, count,
checksum, and duration. Alert on a failed unit or the absence of a successful
monthly completion.

Inspect or run the workflow with:

```bash
sudo systemctl list-timers wareboxes-command-archive.timer
sudo systemctl start wareboxes-command-archive.service
sudo journalctl -u wareboxes-command-archive.service
sudo bash -c 'set -a; . /etc/wareboxes/backup.env; set +a; restic snapshots --tag wareboxes-command-archive'
```

`WAREBOXES_COMMAND_ARCHIVE_AFTER_DAYS` and
`WAREBOXES_COMMAND_ARCHIVE_KEEP_MONTHLY` control the cutoff and snapshot retention.
The restic repository must be off-host before production data is accepted, as
described in [backup-restore.md](backup-restore.md).

## Recovery and retention boundary

An archive is an independently verifiable audit and recovery artifact, not the
online replay source. To inspect a snapshot without placing command results on
disk, stream the selected path through `restic dump`. Treat the output as
tenant-sensitive operational data.

Canonical command deletion is intentionally unsupported at this milestone. A later
retention-and-purge design must first provide an online replay locator that can
recover the exact original result and actor from archived storage under the command
transaction's availability requirements. Until that design is accepted, keeping
the immutable database row is the invariant-preserving choice.
