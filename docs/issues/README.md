# Open issues

The work queue, reviewed 2026-11-08 against published **0.33.1, build 192**. Each
file is one issue: background, tasks or a test checklist, a "done when" where
it applies, and pointers into the code.
Once the repository is on GitHub, `scripts/file-issues.sh` files them all as
GitHub issues (it skips any already filed), and GitHub becomes the place to
track them.

| # | Issue | Labels |
|---|---|---|
| 01 | [Verify desktop sessions on real machines](01-verify-0.5.0-on-real-machines.md) | testing, **high** |
| 02 | [Get CI green on Linux, Windows and macOS](02-github-and-ci.md) (green since 2026-10-23; Clippy in CI, release secrets and issue filing open) | infra |
| 03 | [macOS: build Slipstream.app on a Mac and test it](03-macos-app.md) (built on GitHub's Macs; sharing and control confirmed on a real Mac 2026-11-08) | macOS |
| 04 | [Windows service: verify UAC, elevated apps and the login screen](04-windows-service.md) (UAC, login screen and Ctrl+Alt+Del confirmed on real Windows 2026-11-08; finer checks open) | Windows, testing |
| 05 | [Code signing, and downloads browsers trust](05-code-signing.md) | Windows, macOS, infra, **high** |
| 06 | [Hardware video encoding](06-hardware-encoding.md) (NVENC on Windows and Linux, VA-API; Media Foundation opt-in; VideoToolbox open) | performance, feature |
| 07 | [Stream the host's audio](07-audio.md) (Windows and Linux done and heard between real machines; Mac hosts left) | feature |
| 08 | [Verify station-saved devices and passwords on the live station](08-address-book.md) (in production use; recorded checklist open) | testing |
| 09 | [Android app](09-android.md) (works on a real phone; phase 1/3 features open) | Android, feature |
| 10 | [Test the X11 host for real](10-x11-host-testing.md) (hosting confirmed 2026-11-08; detailed checklist open) | Linux, testing |
| 11 | [Wayland unattended access after reboot](11-wayland-unattended-after-reboot.md) (survives a reboot on the user's machines; other desktops open) | Linux, testing |
| 12 | [Resume interrupted file transfers](12-resumable-transfers.md) | feature |
| 13 | [ARM builds](13-arm-builds.md) | infra |
| 15 | [Windows: measure direct DXGI capture on real hardware](15-windows-capture-dxgi.md) (implemented in 0.7.1) | Windows, performance |
| 16 | [Verify session windows and support tools on real machines](16-session-tools-real-machines.md) | testing |
| 17 | [Group meetings with incoming calls and mobile Show me](17-group-meetings-and-show-me.md) (implemented in 0.23.0; physical room measurements open) | feature, testing |
| 18 | [Verify remote management on real hardware](18-remote-management.md) (implemented in 0.33.0; restarts confirmed on real machines; Wake-on-LAN, BMC/vPro consoles and Redfish open) | feature, testing |
| 19 | [Subspace Relay: a computer's connection through a phone's mobile data](19-subspace-relay.md) (implemented; real phone and computers to try) | feature, Android, testing |

For meetings, see the [usage guide](../MEETINGS.md).

Start with the current priorities in [status](../STATUS.md#next-steps). The
broad real-machine checks were confirmed by the user on 2026-11-08 (see
status); issues 01, 04, 10, 11 and 16 keep their finer-grained checklists,
and 18 has the remaining Wake-on-LAN and controller checks. Issue 01 retains
the laptop and Debian-server checks; 02 covers GitHub/CI.

Checked tasks describe implemented work or recorded verification. Unchecked
tasks remain open; implementation does not complete a real-device checklist.
Run current verification against the latest published release on both peers.
Historical version numbers identify when a feature was introduced. Existing
filenames are retained so links and issue-filing references stay valid.
