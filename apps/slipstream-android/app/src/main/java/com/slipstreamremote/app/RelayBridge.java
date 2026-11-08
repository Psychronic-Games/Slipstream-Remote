package com.slipstreamremote.app;

/** Subspace Relay's reports to Rust (apps/slipstream-android/src/lib.rs). */
final class RelayBridge {
    private RelayBridge() {}
    /** The group is up: computers join `name` with `passphrase`; the phone is `ownerIp` on it. */
    static native void onRelayGroup(String name, String passphrase, String ownerIp);
    /** The relay stopped: empty when asked to, otherwise why. */
    static native void onRelayStopped(String message);
    /** How many computers are on the group. */
    static native void onRelayClients(int count);
}
