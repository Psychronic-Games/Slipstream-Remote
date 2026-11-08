package com.slipstreamremote.app;

/** Subspace Relay's state for the app's window (apps/slipstream-android/src/lib.rs). */
final class RelayBridge {
    private RelayBridge() {}
    static final String STATUS = "com.slipstreamremote.app.RELAY_STATUS";
    /**
     * `state` is starting, up, off or failed; on the network `name` with
     * `passphrase`, the phone is `ip`; `joined` devices are on it and
     * `linked` computers use the relay, with `connections` open and `up`/`down` bytes.
     */
    static native void onRelayStatus(String state, String name, String passphrase, String ip, int joined,
        long linked, long connections, long up, long down, boolean phoneWifi, String message);

    /** Forwards a status broadcast from the relay's process. */
    static void deliver(android.content.Intent i) {
        onRelayStatus(text(i, "state"), text(i, "name"), text(i, "passphrase"), text(i, "ip"), i.getIntExtra("joined", 0),
            i.getLongExtra("linked", 0), i.getLongExtra("connections", 0), i.getLongExtra("up", 0), i.getLongExtra("down", 0),
            i.getBooleanExtra("phoneWifi", false), text(i, "message"));
    }

    static void failed(String message) {
        onRelayStatus("failed", "", "", "", 0, 0, 0, 0, 0, false, message);
    }

    private static String text(android.content.Intent i, String key) {
        String s = i.getStringExtra(key);
        return s == null ? "" : s;
    }
}
