//! Repair the specific KDE/Qt upgrade mismatch before opening a portal session.
//! A restart is permitted only when the portal reports no sessions or requests.
use std::{path::Path, time::Duration};

use anyhow::{Context, Result, ensure};
use ashpd::zbus::{Connection, fdo::IntrospectableProxy};

const KDE: &str = "plasma-xdg-desktop-portal-kde.service";

async fn systemctl(args: &[&str]) -> Result<String> {
    let output = tokio::time::timeout(Duration::from_secs(10),
        tokio::process::Command::new("systemctl").arg("--user").args(args)
            .kill_on_drop(true).output()).await.context("desktop service command timed out")??;
    ensure!(output.status.success(), "desktop service command failed");
    Ok(String::from_utf8(output.stdout)?)
}

fn loaded_qt_core(maps: &str) -> Option<&Path> {
    maps.lines().filter(|line| line.ends_with(" (deleted)"))
        .filter_map(|line| line.split_whitespace().nth(5))
        .map(Path::new)
        .find(|path| path.file_name().is_some_and(|name| name.to_string_lossy().starts_with("libQt6Core.so.")))
}

fn version_changed(loaded: &Path, installed: &Path) -> bool {
    // The deleted marker alone also appears after same-version reinstalls.
    loaded.file_name().zip(installed.file_name()).is_some_and(|(a, b)| a != b)
}

async fn portal_idle() -> Result<bool> {
    let connection = Connection::session().await?;
    for path in ["/org/freedesktop/portal/desktop/session", "/org/freedesktop/portal/desktop/request"] {
        let proxy = IntrospectableProxy::builder(&connection)
            .destination("org.freedesktop.portal.Desktop")?.path(path)?.build().await?;
        let xml = proxy.introspect().await?;
        // Any child may belong to another application. Fail closed instead of
        // interrupting its screen share, dialog, or other portal interaction.
        if portal_has_children(&xml) {
            return Ok(false);
        }
    }
    Ok(true)
}

fn portal_has_children(xml: &str) -> bool {
    xml.split('<').any(|tag| {
        let Some(tail) = tag.strip_prefix("node").and_then(|s| s.trim_start().strip_prefix("name")) else { return false };
        let Some(value) = tail.trim_start().strip_prefix('=').map(str::trim_start) else { return true };
        let Some(quote) = value.chars().next().filter(|c| *c == '\'' || *c == '"') else { return true };
        let name = value[1..].split(quote).next().unwrap_or_default();
        // Some implementations include the absolute root name. Only relative
        // child names describe other clients' objects.
        !name.starts_with('/')
    })
}

async fn repair_if_idle(
    check: impl std::future::Future<Output = Result<bool>>,
    restart: impl std::future::Future<Output = Result<()>>,
) -> Result<()> {
    let idle = tokio::time::timeout(Duration::from_secs(3), check).await
        .context("The desktop needs refreshing after a system update; its active sharing sessions could not be checked. Sign out and back in")?
        .context("The desktop needs refreshing after a system update; its active sharing sessions could not be checked. Sign out and back in")?;
    ensure!(idle, "Screen sharing needs refreshing after a system update. Close other screen-sharing apps/dialogs and choose Approve now, or sign out and back in");
    tracing::warn!("refreshing idle KDE screen-sharing services after a Qt upgrade");
    restart.await.context("Could not refresh desktop screen sharing after a system update; sign out and back in")
}

pub(crate) async fn prepare() -> Result<()> {
    if !std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default().split(':').any(|d| d.eq_ignore_ascii_case("KDE")) {
        return Ok(());
    }
    // Missing systemd/procfs is normal on some desktops. It disables this
    // narrowly scoped repair; ordinary portal access still works as before.
    let Ok(pid) = systemctl(&["show", KDE, "--property=MainPID", "--value"]).await else { return Ok(()) };
    let Ok(pid) = pid.trim().parse::<u32>() else { return Ok(()) };
    if pid == 0 { return Ok(()) }
    let Ok(maps) = std::fs::read_to_string(format!("/proc/{pid}/maps")) else { return Ok(()) };
    let Some(loaded) = loaded_qt_core(&maps) else { return Ok(()) };
    let Some(parent) = loaded.parent() else { return Ok(()) };
    let Ok(installed) = parent.join("libQt6Core.so.6").canonicalize() else { return Ok(()) };
    if !version_changed(loaded, &installed) { return Ok(()) }
    repair_if_idle(portal_idle(), async {
        systemctl(&["restart", KDE, "xdg-desktop-portal.service"]).await.map(|_| ())
    }).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifies_replaced_qt_but_not_unrelated_deleted_files() {
        let maps = "100-200 r-xp 0 00:01 1 /usr/lib/xdg-desktop-portal-kde (deleted)\n\
                    200-300 r-xp 0 00:01 2 /usr/lib/libQt6Core.so.6.11.2 (deleted)\n\
                    300-400 r-xp 0 00:01 3 /usr/lib/libQt6Core.so.6.11.2 (deleted)\n";
        let loaded = loaded_qt_core(maps).unwrap();
        assert!(version_changed(loaded, Path::new("/usr/lib/libQt6Core.so.6.12.0")));
        assert!(!version_changed(loaded, Path::new("/usr/lib/libQt6Core.so.6.11.2")));
        assert!(loaded_qt_core("100-200 r-xp 0 00:01 1 /usr/lib/libQt6Qml.so.6.11.2 (deleted)").is_none());
        assert!(loaded_qt_core("100-200 r-xp 0 00:01 1 /usr/lib/libQt6Core.so.6.12.0").is_none());
    }

    #[tokio::test]
    async fn other_applications_and_unknown_portal_state_prevent_restart() {
        use std::cell::Cell;
        let restarts = Cell::new(0);
        for state in [Ok(false), Err(anyhow::anyhow!("unavailable bus")), Ok(true)] {
            let allowed = state.as_ref().is_ok_and(|idle| *idle);
            let result = repair_if_idle(async {state}, async { restarts.set(restarts.get()+1); Ok(()) }).await;
            assert_eq!(result.is_ok(), allowed);
            assert_eq!(restarts.get(), usize::from(allowed));
        }
        assert!(!portal_has_children("<node/>"));
        assert!(!portal_has_children("<node name=\"/org/freedesktop/portal/desktop/session\"></node>"));
        assert!(portal_has_children("<node><node name='1_42'/></node>"));
        assert!(portal_has_children("<node><node name = \"1_42\"/></node>"));
    }
}
