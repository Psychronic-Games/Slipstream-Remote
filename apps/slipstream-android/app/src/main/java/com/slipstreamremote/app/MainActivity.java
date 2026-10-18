package com.slipstreamremote.app;

import android.content.Intent;
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

    /** Hands a slipstream:// link to the app (lib.rs). */
    private static native void onLink(String url);

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
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
