package com.slipstreamremote.app;
import android.content.BroadcastReceiver;
import android.content.Context;
import android.content.Intent;
/** The user may decline a pending offer without opening the activity. */
public class MeetingCallReceiver extends BroadcastReceiver {
    private static native void decline(long id);
    @Override public void onReceive(Context context,Intent intent) {
        decline(intent.getLongExtra("call",0));
        context.getSystemService(android.app.NotificationManager.class).cancel(3);
    }
}
