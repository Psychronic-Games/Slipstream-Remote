# Open issues

The work queue, reviewed 2026-10-22 against published **0.24.0, build 127**. Each
file is one issue: background, tasks or a test checklist, a "done when" where
it applies, and pointers into the code.
Once the repository is on GitHub, `scripts/file-issues.sh` files them all as
GitHub issues (it skips any already filed), and GitHub becomes the place to
track them.

| # | Issue | Labels |
|---|---|---|
| 01 | [Verify desktop sessions on real machines](01-verify-0.5.0-on-real-machines.md) | testing, **high** |
| 02 | [Get CI green on Linux, Windows and macOS](02-github-and-ci.md) | infra, **high** |
| 03 | [macOS: build Slipstream.app on a Mac and test it](03-macos-app.md) | macOS, **high** |
| 04 | [Windows service: verify UAC, elevated apps and the login screen](04-windows-service.md) (implemented; device checks open) | Windows, testing |
| 05 | [Code signing, and downloads browsers trust](05-code-signing.md) | Windows, macOS, infra, **high** |
| 06 | [Hardware video encoding](06-hardware-encoding.md) | performance, feature |
| 07 | [Stream the host's audio](07-audio.md) (Windows and Linux done; Mac hosts left) | feature |
| 08 | [Verify station-saved devices and passwords on the live station](08-address-book.md) (implemented; live workflow check open) | testing |
| 09 | [Android app](09-android.md) (in progress) | Android, feature |
| 10 | [Test the X11 host for real](10-x11-host-testing.md) | Linux, testing |
| 11 | [Wayland unattended access after reboot](11-wayland-unattended-after-reboot.md) | Linux, testing |
| 12 | [Resume interrupted file transfers](12-resumable-transfers.md) | feature |
| 13 | [ARM builds](13-arm-builds.md) | infra |
| 15 | [Windows: measure direct DXGI capture on real hardware](15-windows-capture-dxgi.md) (implemented in 0.7.1) | Windows, performance |
| 16 | [Verify session windows and support tools on real machines](16-session-tools-real-machines.md) | testing |
| 17 | [Group meetings with incoming calls and mobile Show me](17-group-meetings-and-show-me.md) (implemented in 0.23.0; physical room measurements open) | feature, testing |

For meetings, see the [usage guide](../MEETINGS.md).

Start with the current priorities in [status](../STATUS.md#next-steps). Issues 04, 09 and 16
cover Windows-service, Android-host and session/support-tool runtime checks;
11 includes Wayland reboot and KDE-upgrade recovery. Issue 01 retains the
laptop and Debian-server checks; 02 and 03 cover GitHub/CI and macOS.

Checked tasks describe implemented work or recorded verification. Unchecked
tasks remain open; implementation does not complete a real-device checklist.
Run current verification against the latest published release on both peers.
Historical version numbers identify when a feature was introduced. Existing
filenames are retained so links and issue-filing references stay valid.
