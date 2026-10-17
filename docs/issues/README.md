# Open issues

The work queue after 0.5.0. Each file is one issue: background, tasks or a
test checklist, a "done when" where it applies, and pointers into the code.
Once the repository is on GitHub, `scripts/file-issues.sh` files them all as
GitHub issues (it skips any already filed), and GitHub becomes the place to
track them.

| # | Issue | Labels |
|---|---|---|
| 01 | [Verify 0.5.0 on real machines](01-verify-0.5.0-on-real-machines.md) | testing, **high** |
| 02 | [Publish to GitHub and get CI green](02-github-and-ci.md) | infra, **high** |
| 03 | [macOS: package Slipstream.app and test it](03-macos-app.md) | macOS, **high** |
| 04 | [Windows service for UAC, elevated apps and the login screen](04-windows-service.md) | Windows, feature |
| 05 | [Code signing (Authenticode, notarisation)](05-code-signing.md) | Windows, macOS, infra |
| 06 | [Hardware video encoding](06-hardware-encoding.md) | performance, feature |
| 07 | [Stream the host's audio](07-audio.md) | feature |
| 08 | [Address book with saved devices](08-address-book.md) | feature |
| 09 | [Android app](09-android.md) | Android, feature |
| 10 | [Test the X11 host for real](10-x11-host-testing.md) | Linux, testing |
| 11 | [Wayland unattended access after reboot](11-wayland-unattended-after-reboot.md) | Linux, testing |
| 12 | [Resume interrupted file transfers](12-resumable-transfers.md) | feature |
| 13 | [ARM builds](13-arm-builds.md) | infra |
| 14 | [Windows .exe version from Cargo.toml](14-version-in-one-place.md) | infra, good first issue |

**Start with 01.** It's a checklist for testing the 0.5.0 fixes on the
laptop and the Debian server. 02 and 03 follow.
