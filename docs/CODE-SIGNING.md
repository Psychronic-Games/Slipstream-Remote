# Code signing policy

Free code signing for the Windows app is provided by
[SignPath.io](https://about.signpath.io/), with a certificate by the
[SignPath Foundation](https://signpath.org/).

## What gets signed

Only apps built by this repository's release workflow
(`.github/workflows/release.yml`) from a version tag on `main`. Every signed
file is built in GitHub Actions from the tagged source, and nothing built
elsewhere is ever submitted for signing.

The macOS app is not notarised yet. It is ad-hoc signed, so macOS asks
users to confirm it under *System Settings → Privacy & Security → Open
Anyway*.

## Team

| Role | Who |
|---|---|
| Committers and reviewers | the maintainer (GITHUB-USERNAME) |
| Approvers (each signing request) | the maintainer (GITHUB-USERNAME) |

Contributions from others are reviewed by the maintainer before they are
merged.

## Update signing

Separately from code signing, every app update is signed with the
project's release key (Ed25519). Apps install only updates signed by the
key they were built with. The private key never leaves the maintainer's
machine; CI only uses its public half. See
[BUILDING.md](BUILDING.md#the-release-signing-key).

## Privacy

Slipstream Remote does not send information to anyone but the station it
is set up for. The station is run by the organisation that provides the
download. The app contacts it to get an ID, to find devices you connect
to, and to check for updates. Remote sessions run end-to-end encrypted
between the two computers. There is no telemetry, and no data goes to the
Slipstream project.
