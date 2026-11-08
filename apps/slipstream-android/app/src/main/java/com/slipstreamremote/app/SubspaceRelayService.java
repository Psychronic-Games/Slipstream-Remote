package com.slipstreamremote.app;

import android.app.*;
import android.content.BroadcastReceiver;
import android.content.Context;
import android.content.Intent;
import android.content.IntentFilter;
import android.content.pm.ServiceInfo;
import android.net.wifi.WifiManager;
import android.net.wifi.p2p.WifiP2pConfig;
import android.net.wifi.p2p.WifiP2pManager;
import android.os.*;

/**
 * Subspace Relay: a Wi-Fi Direct group with the user's name and password,
 * kept up with the app in the background. Computers that join it send their
 * traffic to the relay in Rust (crates/slipstream-relay), which opens their
 * connections over this phone's mobile data. See docs/issues/19. MainActivity
 * starts it only on Android 10+, which lets apps name a Wi-Fi Direct group.
 */
@androidx.annotation.RequiresApi(29)
public class SubspaceRelayService extends Service {
    public static volatile boolean running;
    private WifiP2pManager p2p;
    private WifiP2pManager.Channel channel;
    private PowerManager.WakeLock wake;
    private WifiManager.WifiLock wifi;
    private BroadcastReceiver changes;
    private String title = "Starting…";
    private boolean reported;
    private final Handler main = new Handler(Looper.getMainLooper());
    private HandlerThread worker;
    private Handler work;
    private String groupName = "", groupPassphrase = "", groupIp = "";
    private int joined;


    @Override public IBinder onBind(Intent intent) { return null; }

    @Override public int onStartCommand(Intent intent, int flags, int startId) {
        if (intent == null || "stop".equals(intent.getAction())) {
            teardown("");
            return START_NOT_STICKY;
        }
        if (running) return START_NOT_STICKY;
        running = true;
        reported = false;
        publish("starting", "");
        String name = intent.getStringExtra("name");
        String passphrase = intent.getStringExtra("password");
        String band = intent.getStringExtra("band");
        worker = new HandlerThread("slipstream-relay");
        worker.start();
        work = new Handler(worker.getLooper());
        try {
            startForeground(4, notice(), ServiceInfo.FOREGROUND_SERVICE_TYPE_CONNECTED_DEVICE);
        } catch (RuntimeException error) {
            fail("Android didn't let Subspace Relay start: " + error.getMessage());
            return START_NOT_STICKY;
        }
        // Keep relaying with the screen off: the processor awake, and Wi-Fi
        // in its low-latency mode.
        wake = getSystemService(PowerManager.class).newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "slipstream:relay");
        wake.acquire();
        wifi = getSystemService(WifiManager.class).createWifiLock(WifiManager.WIFI_MODE_FULL_LOW_LATENCY, "slipstream:relay");
        wifi.acquire();

        p2p = getSystemService(WifiP2pManager.class);
        if (p2p == null) { fail("This phone doesn't have Wi-Fi Direct."); return START_NOT_STICKY; }
        channel = p2p.initialize(this, Looper.getMainLooper(), () -> { if (running) fail("Wi-Fi Direct stopped."); });
        changes = new BroadcastReceiver() {
            @Override public void onReceive(Context context, Intent change) {
                if (!running || channel == null) return;
                try {
                    p2p.requestGroupInfo(channel, group -> {
                        if (group != null && group.isGroupOwner()) joined = group.getClientList().size();
                    });
                } catch (SecurityException ignored) {}
            }
        };
        registerReceiver(changes, new IntentFilter(WifiP2pManager.WIFI_P2P_CONNECTION_CHANGED_ACTION), Context.RECEIVER_NOT_EXPORTED);
        // A group left from before (or another app's) would block ours.
        p2p.removeGroup(channel, new WifiP2pManager.ActionListener() {
            @Override public void onSuccess() { create(name, passphrase, band); }
            @Override public void onFailure(int reason) { create(name, passphrase, band); }
        });
        return START_NOT_STICKY;
    }

    private void create(String name, String passphrase, String band) {
        if (!running) return;
        // 2.4 GHz by default: on 5 GHz Android advertises a transmit power
        // limit that makes Linux computers turn their radio almost off.
        int owner = "5".equals(band) ? WifiP2pConfig.GROUP_OWNER_BAND_5GHZ
            : "auto".equals(band) ? WifiP2pConfig.GROUP_OWNER_BAND_AUTO : WifiP2pConfig.GROUP_OWNER_BAND_2GHZ;
        WifiP2pConfig config;
        try {
            config = new WifiP2pConfig.Builder()
                .setNetworkName("DIRECT-ss-" + name)
                .setPassphrase(passphrase)
                .setGroupOperatingBand(owner)
                .build();
        } catch (IllegalArgumentException error) {
            fail("That network name or password isn't allowed: " + error.getMessage());
            return;
        }
        try {
            p2p.createGroup(channel, config, new WifiP2pManager.ActionListener() {
                @Override public void onSuccess() { awaitGroup(40); }
                @Override public void onFailure(int reason) { fail(reasonText(reason)); }
            });
        } catch (SecurityException error) {
            fail("Slipstream needs the Nearby devices permission to start the relay.");
        }
    }

    /** The group takes a moment to form after createGroup succeeds. */
    private void awaitGroup(int tries) {
        if (!running) return;
        try {
            p2p.requestGroupInfo(channel, group -> {
                if (group == null || !group.isGroupOwner()) {
                    if (tries > 0) main.postDelayed(() -> awaitGroup(tries - 1), 250);
                    else fail("The Wi-Fi Direct group didn't start.");
                    return;
                }
                p2p.requestConnectionInfo(channel, info -> {
                    String ip = info != null && info.groupOwnerAddress != null ? info.groupOwnerAddress.getHostAddress() : "192.168.49.1";
                    groupName = group.getNetworkName(); groupPassphrase = group.getPassphrase(); groupIp = ip;
                    joined = group.getClientList().size();
                    title = "Join “" + groupName + "” · password " + groupPassphrase;
                    getSystemService(NotificationManager.class).notify(4, notice());
                    String home = getFilesDir().getPath();
                    work.post(() -> {
                        String error = RelayNative.startServer(this, ip, home);
                        main.post(() -> {
                            if (!running) return;
                            if (error != null && !error.isEmpty()) { fail(error); return; }
                            reported = true;
                            tick();
                        });
                    });
                });
            });
        } catch (SecurityException error) {
            fail("Slipstream needs the Nearby devices permission to start the relay.");
        }
    }

    /** Tells the app's window (in the main process) how the relay is doing, every second while up. */
    private void tick() {
        if (!running || !reported) return;
        publish("up", "");
        main.postDelayed(this::tick, 1000);
    }

    private void publish(String state, String message) {
        Intent status = new Intent(RelayBridge.STATUS).setPackage(getPackageName())
            .putExtra("state", state).putExtra("message", message);
        if ("up".equals(state)) {
            long[] s = RelayNative.stats();
            status.putExtra("name", groupName).putExtra("passphrase", groupPassphrase).putExtra("ip", groupIp)
                .putExtra("joined", joined);
            if (s != null && s.length == 4)
                status.putExtra("linked", s[0]).putExtra("connections", s[1]).putExtra("up", s[2]).putExtra("down", s[3]);
        }
        sendBroadcast(status);
    }

    private static String reasonText(int reason) {
        switch (reason) {
            case WifiP2pManager.P2P_UNSUPPORTED: return "This phone doesn't support Wi-Fi Direct.";
            case WifiP2pManager.BUSY: return "Wi-Fi Direct is busy. Turn Wi-Fi on (it doesn't need to join a network) and try again.";
            default: return "Wi-Fi Direct couldn't start. Turn Wi-Fi on (it doesn't need to join a network) and try again.";
        }
    }

    private Notification notice() {
        NotificationManager manager = getSystemService(NotificationManager.class);
        manager.createNotificationChannel(new NotificationChannel("subspace-relay", "Subspace Relay", NotificationManager.IMPORTANCE_LOW));
        PendingIntent open = PendingIntent.getActivity(this, 8, new Intent(this, MainActivity.class), PendingIntent.FLAG_IMMUTABLE | PendingIntent.FLAG_UPDATE_CURRENT);
        PendingIntent stop = PendingIntent.getService(this, 9, new Intent(this, SubspaceRelayService.class).setAction("stop"), PendingIntent.FLAG_IMMUTABLE | PendingIntent.FLAG_UPDATE_CURRENT);
        return new Notification.Builder(this, "subspace-relay").setSmallIcon(com.slipstreamremote.app.R.drawable.ic_sharing)
            .setContentTitle("Subspace Relay").setContentText(title).setContentIntent(open).setOngoing(true)
            .addAction(new Notification.Action.Builder(null, "Stop relay", stop).build()).build();
    }

    private void fail(String message) {
        teardown(message);
    }

    private void teardown(String message) {
        boolean was = running;
        running = false;
        main.removeCallbacksAndMessages(null);
        if (changes != null) { try { unregisterReceiver(changes); } catch (IllegalArgumentException ignored) {} changes = null; }
        if (p2p != null && channel != null) {
            try { p2p.removeGroup(channel, null); } catch (RuntimeException ignored) {}
            channel.close();
            channel = null;
        }
        if (wifi != null && wifi.isHeld()) wifi.release();
        if (wake != null && wake.isHeld()) wake.release();
        if (work != null) {
            HandlerThread w = worker;
            work.post(() -> { RelayNative.stopServer(); w.quitSafely(); });
            work = null; worker = null;
        }
        if (was || !message.isEmpty()) publish(message.isEmpty() ? "off" : "failed", message);
        stopForeground(STOP_FOREGROUND_REMOVE);
        stopSelf();
    }

    @Override public void onDestroy() {
        if (running) teardown("");
        super.onDestroy();
    }
}
