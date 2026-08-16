#!/usr/bin/env bash
set -euo pipefail

# SSR and hydration are compiled and linted in their dedicated CI lanes. Unit
# tests stay feature-neutral so rustc does not codegen a redundant native SSR UI
# harness whose peak memory exceeds the hosted-runner envelope.
cargo test -p wareboxes-web-ops --lib --locked
