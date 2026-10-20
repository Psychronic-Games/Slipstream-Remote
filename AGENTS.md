# Build and check limits

This workstation has about 30 GiB RAM and 32 logical CPUs. Unbounded builds
exhausted RAM/swap and killed terminal sessions on October 19–20, 2026.

Use `scripts/check.sh COMMAND [ARG ...]` for substantial compilation, tests,
Clippy, native rendering checks and asset generation. With no arguments it
runs workspace tests. It calls `/home/doug/.local/bin/codex-check` on this
workstation to serialize heavy work in an 8 GiB memory / 1 GiB swap cgroup,
with two Cargo jobs. Do not increase jobs or memory limits. Run one heavy
check at a time, in the foreground; use tool sessions and poll long runs.
Do not detach children or append shell `&`. Node suites must use
`node --test --test-concurrency=1`; native render audits start with at most
four cases per batch and close the browser between batches. Avoid assertions
that print large pixel/mesh buffers. If memory limits are hit, inspect and
reduce the workload rather than increasing the limit.
The check script supplies a 1024 MiB Node old-space budget, including Node
launched inside shell/npm commands. Keep that budget for separate invocations.
Temporary memory traces, when enabled, are under
`/home/doug/.local/state/codex-diagnostics/`.

The check wrapper enforces cache budgets before and after each command.
Station and Android build scripts do the same automatically. Keep these hooks:
12 GiB for all development/test caches, 32 GiB for all release caches, and
only the newest two generated Android APKs per variant. Incremental compilation
is disabled by default; do not re-enable it without a specific need and a
bounded retention plan. Run `scripts/clean.sh --status` to inspect sizes.
Only generated local build output is eligible for cleanup; preserve sources,
keys, profiles, assembled station clients and deployed rollback backups.
