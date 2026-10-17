---
title: Take the Windows .exe version from Cargo.toml automatically
labels: [infra, good-first-issue]
---

## Background

The version lives in two places: `version` in the workspace `Cargo.toml`
and `packaging/windows/slipstream.rc` (FILEVERSION and the version
strings). They have to be changed by hand together, and it's easy to
forget one.

## Tasks

- [ ] In `apps/slipstream/build.rs`, pass the version (and the build number
      as the 4th component) to the resource compiler, e.g. as macros
      through `embed_resource::compile(…, &["SLIP_VER=0,5,0,13",
      "SLIP_VER_STR=\"0.5.0\""])`. Or generate the `.rc` into `OUT_DIR`
      from a template.
- [ ] Check with `cargo xwin build`: the file properties show the right
      version.
- [ ] Remove the "keep in step" notes from `slipstream.rc` and
      `docs/BUILDING.md`.

## Pointers

- `apps/slipstream/build.rs`, `packaging/windows/slipstream.rc`
- `crates/slipstream-core/build.rs` (where the build number comes from)
