# PostgreSQL Backup and Restore

Wareboxes takes a daily logical PostgreSQL backup into an encrypted restic
repository and performs a weekly restore into an isolated temporary PostgreSQL
container. The default retention is seven daily, five weekly, and twelve monthly
snapshots. A restore drill must complete within 60 minutes.

This establishes a baseline recovery point objective of 24 hours and a tested
recovery time objective of 60 minutes for the supported baseline facility profile.
Measure both again whenever the operational data envelope changes.

## Production repository

Provisioning creates a local encrypted repository so backup and restore automation
starts immediately. A local repository does not survive loss of the host. Before
accepting production data, replace `RESTIC_REPOSITORY` in
`/etc/wareboxes/backup.env` with an encrypted off-host restic backend and add its
least-privilege credentials to that root-readable file. Keep
`/etc/wareboxes/restic_password` in the deployment secret manager and in a separate
recovery escrow; snapshots cannot be decrypted without it.

After changing the repository, initialize and validate it:

```bash
sudo --preserve-env=RESTIC_REPOSITORY,RESTIC_PASSWORD_FILE restic init
sudo systemctl start wareboxes-backup.service
sudo systemctl start wareboxes-restore-drill.service
sudo journalctl -u wareboxes-backup.service -u wareboxes-restore-drill.service
```

The backup must finish with `event=backup_completed`. The drill must finish with
`event=restore_drill_completed`. Alert on a failed unit, a missing successful daily
backup, or a missing successful weekly drill.

## Inspect and test

```bash
sudo systemctl list-timers wareboxes-backup.timer wareboxes-restore-drill.timer
sudo systemctl start wareboxes-backup.service
sudo systemctl start wareboxes-restore-drill.service
sudo systemctl status wareboxes-backup.service wareboxes-restore-drill.service
sudo bash -c 'set -a; . /etc/wareboxes/backup.env; set +a; restic snapshots'
```

The restore drill never publishes a port and removes its temporary database
container on success or failure. It validates critical tables, forced row-level
security, runtime-role privileges, and the configured recovery-time budget.

Durable command results also have a cumulative, encrypted monthly archive with
independent row-count and checksum verification. Its partitioning, schedule, and
online replay invariants are documented in
[command-archives.md](command-archives.md).

## Disaster restore

Start from a host provisioned with the same `deploy/runtime-version`. Configure its
`backup.env` and restic password to access the off-host repository. If the source
host differs, set `WAREBOXES_BACKUP_SOURCE_HOST` to the host shown by
`restic snapshots`.

Verify the selected snapshot before changing the database:

```bash
sudo systemctl stop wareboxes.service wareboxes-worker.service
sudo bash -c 'set -a; . /etc/wareboxes/backup.env; set +a; wareboxes-restore-postgres --check --snapshot latest'
```

Restore only after the check succeeds. This command irreversibly replaces the
configured `wareboxes` database, so its explicit confirmation is mandatory:

```bash
sudo bash -c 'set -a; . /etc/wareboxes/backup.env; set +a; wareboxes-restore-postgres --snapshot latest --confirm-destroy-database wareboxes'
sudo systemctl start wareboxes.service wareboxes-worker.service
curl --fail --silent --show-error http://127.0.0.1:8080/health
```

Then verify login, inventory reconciliation, outbox delivery, and one read-only
workflow for every active tenant before reopening external traffic. Record the
snapshot ID, measured recovery duration, operator, and incident or exercise ID.
