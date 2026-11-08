package com.slipstreamremote.app;

/** The relay server in Rust (apps/slipstream-android/src/relay.rs), in the relay's own process. */
final class RelayNative {
    static { System.loadLibrary("slipstream_android"); }
    private RelayNative() {}
    /** Starts the relay on the group's address; empty on success, else why. Blocks: not on the main thread. */
    static native String startServer(android.content.Context context, String ip, String home);
    static native void stopServer();
    /** [computers linked, connections, bytes up, bytes down], or empty while no relay runs. */
    static native long[] stats();
}
