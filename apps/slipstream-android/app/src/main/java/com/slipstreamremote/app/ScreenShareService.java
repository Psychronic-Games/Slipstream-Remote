package com.slipstreamremote.app;

import android.app.*;
import android.content.*;
import android.content.pm.ServiceInfo;
import android.graphics.PixelFormat;
import android.hardware.display.*;
import android.media.*;
import android.media.projection.*;
import android.os.*;
import android.util.DisplayMetrics;
import java.nio.ByteBuffer;

/** User-approved capture lives outside the activity and stops from its notification. */
public final class ScreenShareService extends Service {
    static { System.loadLibrary("slipstream_android"); }
    static volatile boolean active;
    private MediaProjection projection;
    private VirtualDisplay display;
    private volatile ImageReader images;
    private HandlerThread worker;
    private Handler frames;
    private int sourceWidth, sourceHeight;
    private long lastFrame;
    private boolean framePending; // Accessed only by the frame handler.
    private static final long FRAME_INTERVAL_NANOS = 33_333_334L;
    private String stopReason = "Screen sharing stopped. Tap Start sharing and approve a new session to reconnect.";
    private final Handler main = new Handler(Looper.getMainLooper());
    private static native void onFrame(ByteBuffer buffer, int width, int height, int rowStride, int pixelStride);
    private static native void onCaptureStarted(int width, int height, boolean fullDisplay);
    private static native void onFullDisplay(boolean fullDisplay);
    private static native void onCaptureStopped(String message);
    private final MediaProjection.Callback callback = new MediaProjection.Callback() {
        @Override public void onStop() { stopSelf(); }
        @Override public void onCapturedContentResize(int width, int height) { resize(width, height); }
    };
    private final DisplayManager.DisplayListener rotations = new DisplayManager.DisplayListener() {
        public void onDisplayAdded(int id) {}
        public void onDisplayRemoved(int id) {}
        public void onDisplayChanged(int id) {
            if (id == android.view.Display.DEFAULT_DISPLAY && Build.VERSION.SDK_INT < 34) {
                DisplayMetrics metrics = metrics(); resize(metrics.widthPixels, metrics.heightPixels);
            }
        }
    };
    private DisplayMetrics metrics() {
        DisplayMetrics metrics = new DisplayMetrics();
        getSystemService(DisplayManager.class).getDisplay(android.view.Display.DEFAULT_DISPLAY).getRealMetrics(metrics);
        return metrics;
    }
    @Override public int onStartCommand(Intent intent, int flags, int startId) {
        if (intent == null || "stop".equals(intent.getAction())) {stopSelf();return START_NOT_STICKY;}
        if (active) return START_NOT_STICKY; // Never reuse a projection token.
        try {
            NotificationManager manager = getSystemService(NotificationManager.class);
            manager.createNotificationChannel(new NotificationChannel("screen-sharing", "Screen sharing", NotificationManager.IMPORTANCE_LOW));
            PendingIntent open = PendingIntent.getActivity(this, 1, new Intent(this, MainActivity.class), PendingIntent.FLAG_IMMUTABLE | PendingIntent.FLAG_UPDATE_CURRENT);
            PendingIntent stop = PendingIntent.getService(this, 2, new Intent(this, ScreenShareService.class).setAction("stop"), PendingIntent.FLAG_IMMUTABLE | PendingIntent.FLAG_UPDATE_CURRENT);
            Notification notice = new Notification.Builder(this, "screen-sharing")
                .setSmallIcon(com.slipstreamremote.app.R.drawable.ic_sharing)
                .setContentTitle("Slipstream · screen sharing")
                .setContentText("Authorized computers can view this phone. Tap Stop sharing to end access.")
                .setContentIntent(open).setOngoing(true)
                .addAction(new Notification.Action.Builder(null, "Stop sharing", stop).build()).build();
            if (Build.VERSION.SDK_INT >= 29) startForeground(1, notice, ServiceInfo.FOREGROUND_SERVICE_TYPE_MEDIA_PROJECTION);
            else startForeground(1, notice);
            Intent consent;
            if (Build.VERSION.SDK_INT >= 33) consent = intent.getParcelableExtra("consent", Intent.class);
            else consent = intent.getParcelableExtra("consent");
            if (consent == null) throw new IllegalStateException("Screen-sharing approval was not provided");
            projection = getSystemService(MediaProjectionManager.class).getMediaProjection(Activity.RESULT_OK, consent);
            projection.registerCallback(callback, main);
            worker = new HandlerThread("slipstream-screen");worker.start();frames = new Handler(worker.getLooper());
            DisplayMetrics metrics = metrics();sourceWidth = metrics.widthPixels;sourceHeight = metrics.heightPixels;
            int[] size = captureSize(sourceWidth, sourceHeight);
            images = reader(size[0], size[1]);active = true;
            onCaptureStarted(size[0], size[1], true);
            display = projection.createVirtualDisplay("Slipstream Remote", size[0], size[1], getResources().getConfiguration().densityDpi,
                DisplayManager.VIRTUAL_DISPLAY_FLAG_AUTO_MIRROR, images.getSurface(), null, main);
            getSystemService(DisplayManager.class).registerDisplayListener(rotations, main);
            // Sharing is ready before leaving the activity. Showing the
            // phone's other task also avoids starting with a recursive app view.
            MainActivity.sharingStarted();
        } catch (RuntimeException error) {
            stopReason = "Could not share screen: " + error.getClass().getSimpleName() + ". Tap Start sharing to try again.";
            stopSelf();
        }
        return START_NOT_STICKY;
    }
    // At most 1280 on the long edge. Use the existing H.264 pipeline without
    // holding a full phone-resolution buffer or a queue of stale frames.
    private static int[] captureSize(int width, int height) {
        float scale = Math.min(1f, 1280f / Math.max(width, height));
        return new int[]{Math.max(2, ((int)(width * scale)) & ~1), Math.max(2, ((int)(height * scale)) & ~1)};
    }
    private ImageReader reader(int width, int height) {
        ImageReader reader = ImageReader.newInstance(width, height, PixelFormat.RGBA_8888, 2);
        reader.setOnImageAvailableListener(source -> {
            if (source != images || !active || framePending) return;
            // Do not acquire/discard an early frame: it may be the last image
            // of a now-static screen. Drain the newest image when it is due.
            framePending = true;
            long wait = Math.max(0, FRAME_INTERVAL_NANOS - (SystemClock.elapsedRealtimeNanos() - lastFrame));
            frames.postDelayed(this::drainFrame, (wait + 999_999L) / 1_000_000L);
        }, frames);
        return reader;
    }
    private void drainFrame() {
        framePending = false;
        ImageReader source = images;
        if (source == null || !active) return;
        try (Image image = source.acquireLatestImage()) {
            if (image == null) return;
            lastFrame = SystemClock.elapsedRealtimeNanos();
            Image.Plane plane = image.getPlanes()[0];
            onFrame(plane.getBuffer(), image.getWidth(), image.getHeight(), plane.getRowStride(), plane.getPixelStride());
        } catch (IllegalStateException ignored) { /* Sharing ended or the reader was replaced. */ }
    }
    private void resize(int width, int height) {
        if (!active || display == null || width <= 0 || height <= 0) return;
        DisplayMetrics metrics = metrics();
        boolean full = width == metrics.widthPixels && height == metrics.heightPixels;
        onFullDisplay(full);
        if (!full) RemoteInputService.reset();
        if (sourceWidth == width && sourceHeight == height) return;
        RemoteInputService.reset();
        sourceWidth = width;sourceHeight = height;
        int[] size = captureSize(width, height);
        ImageReader old = images;
        images = reader(size[0], size[1]);
        display.setSurface(images.getSurface());
        display.resize(size[0], size[1], getResources().getConfiguration().densityDpi);
        if (old != null) frames.post(old::close);
    }
    @Override public void onDestroy() {
        active = false;
        RemoteInputService.reset();
        onCaptureStopped(stopReason);
        getSystemService(DisplayManager.class).unregisterDisplayListener(rotations);
        if (display != null) {display.release();display = null;}
        if (projection != null) {projection.unregisterCallback(callback);projection.stop();projection = null;}
        ImageReader old = images;images = null;if (old != null) frames.post(old::close);
        if (worker != null) worker.quitSafely();
        stopForeground(STOP_FOREGROUND_REMOVE);
        super.onDestroy();
    }
    @Override public IBinder onBind(Intent intent) { return null; }
}
