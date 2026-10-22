# Code signing policy

*Current status: 2026-10-21, published release 0.23.2.*

Desktop update signing is active. Windows publisher signing through
[SignPath](https://signpath.org/) is planned; the repository has no GitHub
remote configured, and the CI/release workflows have not run. SignPath
acceptance and credentials are still pending. The published Windows executable
is not Authenticode-signed and can trigger SmartScreen warnings.

## Current signing

| Artifact | Signing and verification |
|---|---|
| Linux and Windows desktop updates | Ed25519 signatures in the update manifest, checked against release public keys built into the apps. This is separate from OS publisher signing. |
| Android release APK | Signed with the existing Android release certificate. Users install updates from the station; Android self-update is not implemented. Debuggable builds use a separate debug key. |
| macOS app | The packaging script produces an ad-hoc-signed bundle. Developer ID signing, notarisation, and native build/runtime verification remain pending. |

See the [latest handoff](DEVELOPMENT-2026-10-21.md) for published artifact
verification, and [BUILDING.md](BUILDING.md#the-release-signing-key) for key
storage and backup instructions. A plain Cargo build without configured update
public keys cannot install signed updates.

## Planned Windows publisher signing

The intended policy is to submit only artifacts built by this repository's
GitHub Actions release workflow from reviewed source on `main`. The workflow
currently triggers on `v*` tags; it does not enforce that a tag belongs to
`main`. Verify that before submitting a signing request.

`.github/workflows/release.yml` includes a conditional SignPath step. It runs
only when `SIGNPATH_ORGANIZATION_ID` is configured and also needs the project,
policy and API-token settings listed in the workflow. Without signing configured,
the workflow retains the unsigned Windows executable. It does not currently
provide a signed Windows release.

Before the first signing request, publish the repository, complete the SignPath
application, configure the workflow and name the actual GitHub reviewers and
signing approvers in this policy. See [issue 05](issues/05-code-signing.md).

## Team

| Role | Who |
|---|---|
| Committers and reviewers | Project maintainer; GitHub identity pending publication |
| Approvers (each future Windows signing request) | Project maintainer; GitHub identity pending publication |

Contributions from others are reviewed by the maintainer before they are
merged.

## Update signing

Separately from code signing, every desktop update is signed with the
project's release key (Ed25519). Apps install only updates signed by a
trusted key built into them. The private key stays outside the repository
and is not uploaded to the station or CI; the planned CI workflow uses
only public keys. Station assembly signs the update manifest locally. See
[BUILDING.md](BUILDING.md#the-release-signing-key).

## Privacy

The configured station handles directory registration, device lookup,
technician accounts and update checks/downloads. Remote sessions run
end-to-end encrypted between devices, directly or through relays. Public
fallback relays can be used when allowed by both the station and client.

Opening System information or collecting a support report can request the
remote host's public IP from `api.ipify.org`, with `api64.ipify.org` as a
fallback. Those requests contain no station key, device ID or account data.
Network scans and diagnostics contact their selected network targets, DNS
resolvers and station. There is no telemetry sent to the Slipstream project.
