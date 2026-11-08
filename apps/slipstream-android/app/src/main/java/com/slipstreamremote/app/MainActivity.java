package com.slipstreamremote.app;

import android.content.Intent;
import android.media.projection.MediaProjectionManager;
import android.media.projection.MediaProjectionConfig;
import android.os.Build;
import android.provider.Settings;
import java.lang.ref.WeakReference;
import android.net.Uri;
import android.os.Bundle;
import android.view.View;

import androidx.core.graphics.Insets;
import androidx.core.view.ViewCompat;
import androidx.core.view.WindowInsetsCompat;

import com.google.androidgamesdk.GameActivity;

/** The app: the Rust GUI (apps/slipstream-android/src/lib.rs) in a GameActivity. */
public class MainActivity extends GameActivity {
    static {
        System.loadLibrary("slipstream_android");
    }

    private static WeakReference<MainActivity> current = new WeakReference<>(null);
    private boolean requestingShare;
    private int pendingMeetingAction = -1;
    private boolean resumed;
    public static boolean microphoneReady() { return MeetingCaptureService.microphoneActive; }
    public static void meetingCall(String name,long id) {
        MainActivity activity = current.get();
        if (activity == null) return;
        activity.runOnUiThread(() -> {
            android.app.NotificationManager manager = activity.getSystemService(android.app.NotificationManager.class);
            if (name.isEmpty()) {manager.cancel(3);return;}
            if (Build.VERSION.SDK_INT >= 33 && activity.checkSelfPermission(android.Manifest.permission.POST_NOTIFICATIONS) != android.content.pm.PackageManager.PERMISSION_GRANTED) return;
            manager.createNotificationChannel(new android.app.NotificationChannel("incoming-meeting","Incoming meeting calls",android.app.NotificationManager.IMPORTANCE_HIGH));
            android.app.PendingIntent open = android.app.PendingIntent.getActivity(activity,6,new Intent(activity,MainActivity.class),android.app.PendingIntent.FLAG_IMMUTABLE | android.app.PendingIntent.FLAG_UPDATE_CURRENT);
            android.app.PendingIntent decline = android.app.PendingIntent.getBroadcast(activity,7,new Intent(activity,MeetingCallReceiver.class).putExtra("call",id),android.app.PendingIntent.FLAG_IMMUTABLE | android.app.PendingIntent.FLAG_UPDATE_CURRENT);
            manager.notify(3,new android.app.Notification.Builder(activity,"incoming-meeting").setSmallIcon(com.slipstreamremote.app.R.drawable.ic_sharing)
                .setContentTitle(name + " is calling").setContentText("Tap to answer in Slipstream").setContentIntent(open)
                .setCategory(android.app.Notification.CATEGORY_CALL).setTimeoutAfter(35_000).setAutoCancel(true)
                .addAction(new android.app.Notification.Action.Builder(null,"Decline",decline).build()).build());
        });
    }
    public static String deviceName() { return Build.MANUFACTURER + " " + Build.MODEL; }
    public static void hostAction(int action) {
        MainActivity activity = current.get();
        if (activity == null) return;
        activity.runOnUiThread(() -> {
            if (action >= 10) activity.meetingAction(action);
            else if (action == 0) activity.startActivity(new Intent(Settings.ACTION_ACCESSIBILITY_SETTINGS));
            else if (action == 2) activity.stopService(new Intent(activity,ScreenShareService.class));
            else if (action == 3) activity.startActivity(new Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS,Uri.parse("package:"+activity.getPackageName())));
            else if (action == 1 && !ScreenShareService.active && !activity.requestingShare) {
                activity.requestingShare = true;
                if (Build.VERSION.SDK_INT >= 33 && activity.checkSelfPermission(android.Manifest.permission.POST_NOTIFICATIONS) != android.content.pm.PackageManager.PERMISSION_GRANTED)
                    activity.requestPermissions(new String[]{android.Manifest.permission.POST_NOTIFICATIONS},1002);
                else activity.requestProjection();
            }
        });
    }
    private void meetingAction(int action) {
        if (action == 16) {
            if (Build.VERSION.SDK_INT >= 33 && checkSelfPermission(android.Manifest.permission.POST_NOTIFICATIONS) != android.content.pm.PackageManager.PERMISSION_GRANTED)
                requestPermissions(new String[]{android.Manifest.permission.POST_NOTIFICATIONS},1004);
            return;
        }
        if (action == 12 || action == 15 || action == 13) {
            if ((action == 12 && (pendingMeetingAction == 10 || pendingMeetingAction == 11)) || (action == 15 && pendingMeetingAction == 14)) pendingMeetingAction = -1;
            if (MeetingCaptureService.running)
                startService(new Intent(this,MeetingCaptureService.class).putExtra("action",action));
            return;
        }
        if (!resumed) return; // Camera/microphone start needs a visible user action.
        String permission = action == 14 ? android.Manifest.permission.RECORD_AUDIO : android.Manifest.permission.CAMERA;
        if (checkSelfPermission(permission) != android.content.pm.PackageManager.PERMISSION_GRANTED) {
            pendingMeetingAction = action;
            requestPermissions(new String[]{permission},1003);
        } else startForegroundService(new Intent(this,MeetingCaptureService.class).putExtra("action",action));
    }
    private void resumeMeetingAction() {
        int action = pendingMeetingAction;
        if (!resumed || action < 10) return;
        String permission = action == 14 ? android.Manifest.permission.RECORD_AUDIO : android.Manifest.permission.CAMERA;
        if (checkSelfPermission(permission) == android.content.pm.PackageManager.PERMISSION_GRANTED) {
            pendingMeetingAction = -1;
            meetingAction(action);
        }
    }
    @Override protected void onResume() { super.onResume(); resumed = true; resumeMeetingAction(); }
    @Override protected void onPause() { resumed = false; super.onPause(); }
    public static void sharingStarted() {
        MainActivity activity = current.get();
        if (activity != null && !activity.isFinishing()) activity.moveTaskToBack(true);
    }
    /** Subspace Relay from Rust: 1 starts it with this name, password and band, 0 stops it. */
    public static void relayAction(int action,String name,String password,String band) {
        MainActivity activity = current.get();
        if (activity == null) return;
        activity.runOnUiThread(() -> {
            if (action == 0) {
                activity.startService(new Intent(activity,SubspaceRelayService.class).setAction("stop"));
                return;
            }
            if (Build.VERSION.SDK_INT < 29) {RelayBridge.failed("Subspace Relay needs Android 10 or newer.");return;}
            activity.relayName = name; activity.relayPassword = password; activity.relayBand = band;
            String[] needed = activity.relayPermissionsMissing();
            if (needed.length > 0) activity.requestPermissions(needed,1005);
            else activity.startRelay();
        });
    }
    private String relayName, relayPassword, relayBand;
    private android.content.BroadcastReceiver relayStatus;
    /** Starts the app again in a new process (lib.rs: this one can't show the window again). */
    public void relaunch() {
        Intent intent = getPackageManager().getLaunchIntentForPackage(getPackageName());
        if (intent == null) return;
        intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK | Intent.FLAG_ACTIVITY_CLEAR_TASK);
        startActivity(intent);
    }
    private String[] relayPermissionsMissing() {
        java.util.List<String> missing = new java.util.ArrayList<>();
        java.util.List<String> wanted = new java.util.ArrayList<>();
        if (Build.VERSION.SDK_INT >= 33) {
            wanted.add(android.Manifest.permission.NEARBY_WIFI_DEVICES);
            wanted.add(android.Manifest.permission.POST_NOTIFICATIONS);
        } else wanted.add(android.Manifest.permission.ACCESS_FINE_LOCATION);
        for (String p : wanted) if (checkSelfPermission(p) != android.content.pm.PackageManager.PERMISSION_GRANTED) missing.add(p);
        return missing.toArray(new String[0]);
    }
    private void startRelay() {
        if (relayName == null) return;
        startForegroundService(new Intent(this,SubspaceRelayService.class).putExtra("name",relayName).putExtra("password",relayPassword).putExtra("band",relayBand));
    }
    public static void remoteInput(int kind,float x,float y,String key,boolean down) { RemoteInputService.dispatch(kind,x,y,key,down); }
    private void requestProjection() {
        MediaProjectionManager manager = getSystemService(MediaProjectionManager.class);
        Intent request = Build.VERSION.SDK_INT >= 34
            ? manager.createScreenCaptureIntent(MediaProjectionConfig.createConfigForDefaultDisplay())
            : manager.createScreenCaptureIntent();
        startActivityForResult(request,1001);
    }
    @Override public void onRequestPermissionsResult(int request,String[] permissions,int[] results) {
        super.onRequestPermissionsResult(request,permissions,results);
        if (request == 1002) requestProjection();
        if (request == 1005) {
            // Notifications are optional; Wi-Fi Direct (nearby devices or, before Android 13, location) isn't.
            boolean wifiDirect = true;
            for (int i = 0; i < permissions.length && i < results.length; i++)
                if (!android.Manifest.permission.POST_NOTIFICATIONS.equals(permissions[i]) && results[i] != android.content.pm.PackageManager.PERMISSION_GRANTED) wifiDirect = false;
            if (wifiDirect && results.length > 0) startRelay();
            else RelayBridge.failed(Build.VERSION.SDK_INT >= 33
                ? "Subspace Relay needs the Nearby devices permission to start a Wi-Fi Direct network."
                : "Subspace Relay needs the Location permission to start a Wi-Fi Direct network (Android uses it for Wi-Fi Direct).");
        }
        if (request == 1003) {
            int action = pendingMeetingAction; pendingMeetingAction = -1;
            if (action >= 10 && results.length > 0 && results[0] == android.content.pm.PackageManager.PERMISSION_GRANTED) {
                pendingMeetingAction = action;
                resumeMeetingAction();
            }
        }
    }
    @Override protected void onActivityResult(int request,int result,Intent data) {
        super.onActivityResult(request,result,data);
        if (request == 1001) {
            requestingShare = false;
            if (result == RESULT_OK && data != null)
                startForegroundService(new Intent(this,ScreenShareService.class).putExtra("consent",data));
        }
    }

    /** Hands a slipstream:// link to the app (lib.rs). */
    private static native void onLink(String url);

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        current = new WeakReference<>(this);
        reportInsets();
        // Subspace Relay runs in its own process and reports here.
        relayStatus = new android.content.BroadcastReceiver() {
            @Override public void onReceive(android.content.Context context, Intent status) { RelayBridge.deliver(status); }
        };
        android.content.IntentFilter filter = new android.content.IntentFilter(RelayBridge.STATUS);
        if (Build.VERSION.SDK_INT >= 33) registerReceiver(relayStatus, filter, RECEIVER_NOT_EXPORTED);
        else registerReceiver(relayStatus, filter);
        // Focused from the start: otherwise the first tap only focuses the
        // view (touch mode), and the app's request for the keyboard, made
        // as that tap focuses a text field, is ignored.
        mSurfaceView.setFocusableInTouchMode(true);
        mSurfaceView.requestFocus();
        // Not when Android recreates the activity: that link was handled.
        if (savedInstanceState == null) {
            passLink(getIntent());
        }
    }

    @Override protected void onDestroy() {
        if (isFinishing()) {
            stopService(new Intent(this,ScreenShareService.class));
            stopService(new Intent(this,MeetingCaptureService.class));
        }
        // Subspace Relay keeps running until it's stopped (its notification has Stop).
        if (relayStatus != null) unregisterReceiver(relayStatus);
        super.onDestroy();
    }

    @Override
    protected void onNewIntent(Intent intent) {
        super.onNewIntent(intent);
        setIntent(intent);
        passLink(intent);
    }

    /** Enter or Done on the soft keyboard (lib.rs). */
    private static native void onImeAction();

    @Override
    public void onEditorAction(int action) {
        onImeAction();
        super.onEditorAction(action);
    }

    /** The screen's covered edges changed, in pixels (lib.rs). */
    private static native void onInsets(int left, int top, int right, int bottom);

    /**
     * Android 15+ draws apps edge to edge, under the status bar, the
     * navigation bar and the camera cutout. The app keeps its content clear
     * of them, and of the soft keyboard while it's open.
     */
    private void reportInsets() {
        View content = findViewById(android.R.id.content);
        int types = WindowInsetsCompat.Type.systemBars() | WindowInsetsCompat.Type.displayCutout() | WindowInsetsCompat.Type.ime();
        ViewCompat.setOnApplyWindowInsetsListener(content, (view, insets) -> {
            Insets covered = insets.getInsets(types);
            onInsets(covered.left, covered.top, covered.right, covered.bottom);
            return insets;
        });
    }

    private static void passLink(Intent intent) {
        Uri data = intent == null ? null : intent.getData();
        if (data != null && "slipstream".equals(data.getScheme())) {
            onLink(data.toString());
        }
    }
}
