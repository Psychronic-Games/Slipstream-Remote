# Open issues

The work queue, reviewed 2026-10-19 during 0.21.0 release preparation. Each
file is one issue: background, tasks or a test checklist, a "done when" where
it applies, and pointers into the code.
Once the repository is on GitHub, `scripts/file-issues.sh` files them all as
GitHub issues (it skips any already filed), and GitHub becomes the place to
track them.

| # | Issue | Labels |
|---|---|---|
| 01 | [Verify 0.5.0 on real machines](01-verify-0.5.0-on-real-machines.md) | testing, **high** |
| 02 | [Publish to GitHub and get CI green](02-github-and-ci.md) | infra, **high** |
| 03 | [macOS: build Slipstream.app on a Mac and test it](03-macos-app.md) | macOS, **high** |
| 04 | [Windows service for UAC, elevated apps and the login screen](04-windows-service.md) | Windows, feature |
| 05 | [Code signing, and downloads browsers trust](05-code-signing.md) | Windows, macOS, infra, **high** |
| 06 | [Hardware video encoding](06-hardware-encoding.md) | performance, feature |
| 07 | [Stream the host's audio](07-audio.md) (Windows and Linux done; Mac hosts left) | feature |
| 08 | [Address book with saved devices](08-address-book.md) (done as technician accounts on stations) | feature |
| 09 | [Android app](09-android.md) (in progress) | Android, feature |
| 10 | [Test the X11 host for real](10-x11-host-testing.md) | Linux, testing |
| 11 | [Wayland unattended access after reboot](11-wayland-unattended-after-reboot.md) | Linux, testing |
| 12 | [Resume interrupted file transfers](12-resumable-transfers.md) | feature |
| 13 | [ARM builds](13-arm-builds.md) | infra |
| 15 | [Windows: capture straight from DXGI](15-windows-capture-dxgi.md) (done in 0.7.1; to measure) | Windows, performance |
| 16 | [Try the new session tools on real machines](16-session-tools-real-machines.md) | testing |

Start with the current priorities in [status](../STATUS.md#next-steps-suggested-order)
and the [2026-10-19 handoff](../DEVELOPMENT-2026-10-19.md). Issues 09 and 16
cover the latest Android-host and Windows-service/tool runtime checks. Issue 01
retains older checks for the laptop and Debian server; 02 and 03 cover GitHub/CI
and macOS. Original version numbers in older checklists describe when the work
was introduced, rather than the latest published client.
