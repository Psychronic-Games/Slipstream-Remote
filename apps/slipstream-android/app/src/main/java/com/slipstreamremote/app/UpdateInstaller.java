package com.slipstreamremote.app;

import android.app.PendingIntent;
import android.content.BroadcastReceiver;
import android.content.Context;
import android.content.Intent;
import android.content.pm.PackageInstaller;
import android.os.Build;
import java.io.File;
import java.io.FileInputStream;
import java.io.InputStream;
import java.io.OutputStream;

/**
 * Installs an update the app downloaded and verified against the station's
 * signed manifest (crates/slipstream-session/src/update.rs). Android checks
 * the APK is signed with the same key as the installed app, and asks the
 * person to confirm.
 */
public class UpdateInstaller extends BroadcastReceiver {
    /** Progress for the app's window: empty when Android took over, else what went wrong. */
    static native void onInstallStatus(String message);

    /** Writes the APK into an installer session and asks Android to install it. */
    static void install(Context context, String path) {
        File apk = new File(path);
        PackageInstaller installer = context.getPackageManager().getPackageInstaller();
        PackageInstaller.SessionParams params = new PackageInstaller.SessionParams(PackageInstaller.SessionParams.MODE_FULL_INSTALL);
        params.setAppPackageName(context.getPackageName());
        params.setSize(apk.length());
        if (Build.VERSION.SDK_INT >= 31) params.setRequireUserAction(PackageInstaller.SessionParams.USER_ACTION_NOT_REQUIRED);
        int id = -1;
        try {
            id = installer.createSession(params);
            try (PackageInstaller.Session session = installer.openSession(id);
                 InputStream in = new FileInputStream(apk);
                 OutputStream out = session.openWrite("slipstream.apk", 0, apk.length())) {
                byte[] buffer = new byte[1 << 16];
                for (int n; (n = in.read(buffer)) > 0; ) out.write(buffer, 0, n);
                session.fsync(out);
                out.close();
                Intent result = new Intent(context, UpdateInstaller.class).setPackage(context.getPackageName());
                int flags = PendingIntent.FLAG_UPDATE_CURRENT | (Build.VERSION.SDK_INT >= 31 ? PendingIntent.FLAG_MUTABLE : 0);
                session.commit(PendingIntent.getBroadcast(context, id, result, flags).getIntentSender());
            }
            onInstallStatus("");
        } catch (Exception error) {
            if (id >= 0) try { installer.abandonSession(id); } catch (RuntimeException ignored) {}
            onInstallStatus("Android couldn't start the installation: " + error.getMessage());
        } finally {
            // The session holds its own copy now.
            if (!apk.delete()) apk.deleteOnExit();
        }
    }

    /** Android's answer: ask the person to confirm, or report why it failed. */
    @Override public void onReceive(Context context, Intent intent) {
        int status = intent.getIntExtra(PackageInstaller.EXTRA_STATUS, PackageInstaller.STATUS_FAILURE);
        if (status == PackageInstaller.STATUS_PENDING_USER_ACTION) {
            @SuppressWarnings("deprecation")
            Intent confirm = intent.getParcelableExtra(Intent.EXTRA_INTENT);
            if (confirm != null) {
                confirm.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK);
                context.startActivity(confirm);
            }
        } else if (status != PackageInstaller.STATUS_SUCCESS) {
            String message = intent.getStringExtra(PackageInstaller.EXTRA_STATUS_MESSAGE);
            onInstallStatus(status == PackageInstaller.STATUS_FAILURE_ABORTED ? "The update was cancelled."
                : "Android didn't install the update" + (message == null ? "." : ": " + message));
        }
        // On success Android replaces the app and closes it; it reopens on the new version.
    }
}
