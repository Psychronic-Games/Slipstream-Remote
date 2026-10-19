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
    public static String deviceName() { return Build.MANUFACTURER + " " + Build.MODEL; }
    public static void hostAction(int action) {
        MainActivity activity = current.get();
        if (activity == null) return;
        activity.runOnUiThread(() -> {
            if (action == 0) activity.startActivity(new Intent(Settings.ACTION_ACCESSIBILITY_SETTINGS));
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
    public static void sharingStarted() {
        MainActivity activity = current.get();
        if (activity != null && !activity.isFinishing()) activity.moveTaskToBack(true);
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
        if (isFinishing()) stopService(new Intent(this,ScreenShareService.class));
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
