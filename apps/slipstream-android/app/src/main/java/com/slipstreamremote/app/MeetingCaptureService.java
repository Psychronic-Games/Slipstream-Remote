package com.slipstreamremote.app;

import android.app.*;
import android.content.Intent;
import android.content.pm.ServiceInfo;
import android.graphics.ImageFormat;
import android.hardware.camera2.*;
import android.hardware.camera2.params.StreamConfigurationMap;
import android.media.*;
import android.os.*;
import android.util.Size;
import android.view.Surface;
import java.nio.ByteBuffer;
import java.util.Arrays;

/** Camera and microphone lifetime follows explicit local meeting controls. */
public class MeetingCaptureService extends Service {
    public static volatile boolean running, microphoneActive;
    private HandlerThread worker;
    private Handler frames;
    private CameraDevice camera;
    private CameraCaptureSession session;
    private ImageReader reader;
    private CaptureRequest.Builder request;
    private volatile boolean cameraWanted;
    private boolean torch, flashSupported;
    private int facing;
    private volatile int generation;
    private long lastFrame;
    private final Handler main = new Handler(Looper.getMainLooper());
    private static native void onCameraStarted(int width,int height);
    private static native void onCameraFrame(ByteBuffer y,ByteBuffer u,ByteBuffer v,int width,int height,
        int yRow,int uvRow,int uvPixel,int rotation);
    private static native void onCameraStopped();
    private static native void onMicrophoneStopped();
    @Override public IBinder onBind(Intent intent) { return null; }
    @Override public int onStartCommand(Intent intent,int flags,int startId) {
        if (intent == null || "stop".equals(intent.getAction())) {stopSelf();return START_NOT_STICKY;}
        running = true;
        int action = intent.getIntExtra("action",12);
        if (action == 14) microphoneActive = true;
        if (action == 15) {microphoneActive = false;onMicrophoneStopped();}
        if (action == 12) {cameraWanted = false;closeCamera();}
        if (action == 10 || action == 11) {cameraWanted = true;facing = action == 11 ? CameraCharacteristics.LENS_FACING_FRONT : CameraCharacteristics.LENS_FACING_BACK;}
        if (action == 13 && flashSupported && request != null && session != null) try {
            torch = !torch;request.set(CaptureRequest.FLASH_MODE,torch ? CaptureRequest.FLASH_MODE_TORCH : CaptureRequest.FLASH_MODE_OFF);
            session.setRepeatingRequest(request.build(),null,frames);
        } catch (CameraAccessException | IllegalArgumentException error) {torch = false;}
        if (!cameraWanted && !microphoneActive) {stopSelf();return START_NOT_STICKY;}
        try {
            NotificationManager manager = getSystemService(NotificationManager.class);
            manager.createNotificationChannel(new NotificationChannel("meeting-media","Meeting camera and microphone",NotificationManager.IMPORTANCE_LOW));
            PendingIntent open = PendingIntent.getActivity(this,4,new Intent(this,MainActivity.class),PendingIntent.FLAG_IMMUTABLE | PendingIntent.FLAG_UPDATE_CURRENT);
            PendingIntent stop = PendingIntent.getService(this,5,new Intent(this,MeetingCaptureService.class).setAction("stop"),PendingIntent.FLAG_IMMUTABLE | PendingIntent.FLAG_UPDATE_CURRENT);
            Notification notice = new Notification.Builder(this,"meeting-media").setSmallIcon(com.slipstreamremote.app.R.drawable.ic_sharing)
                .setContentTitle("Slipstream meeting").setContentText(cameraWanted ? "Camera sharing active" : "Microphone active")
                .setContentIntent(open).setOngoing(true).addAction(new Notification.Action.Builder(null,"Stop camera and microphone",stop).build()).build();
            int types = (cameraWanted ? ServiceInfo.FOREGROUND_SERVICE_TYPE_CAMERA : 0) | (microphoneActive ? ServiceInfo.FOREGROUND_SERVICE_TYPE_MICROPHONE : 0);
            if (Build.VERSION.SDK_INT >= 29) startForeground(2,notice,types); else startForeground(2,notice);
            if (action == 10 || action == 11) openCamera();
        } catch (RuntimeException error) {stopSelf();}
        return START_NOT_STICKY;
    }
    private void openCamera() {
        closeCamera();cameraWanted = true;
        if (worker == null) {worker = new HandlerThread("slipstream-camera");worker.start();frames = new Handler(worker.getLooper());}
        int epoch = ++generation;
        try {
            CameraManager manager = getSystemService(CameraManager.class);
            String selected = null;
            for (String id : manager.getCameraIdList()) {
                Integer lens = manager.getCameraCharacteristics(id).get(CameraCharacteristics.LENS_FACING);
                if (lens != null && lens == facing) {selected = id;break;}
            }
            if (selected == null) throw new IllegalStateException("Camera is unavailable");
            CameraCharacteristics info = manager.getCameraCharacteristics(selected);
            StreamConfigurationMap map = info.get(CameraCharacteristics.SCALER_STREAM_CONFIGURATION_MAP);
            Size[] sizes = map.getOutputSizes(ImageFormat.YUV_420_888);
            Size size = Arrays.stream(sizes).filter(s -> s.getWidth() <= 1280 && s.getHeight() <= 720)
                .min(java.util.Comparator.comparingLong(s -> Math.abs((long)s.getWidth()*s.getHeight()-640*480))).orElseThrow(() -> new IllegalStateException("No supported camera size"));
            flashSupported = Boolean.TRUE.equals(info.get(CameraCharacteristics.FLASH_INFO_AVAILABLE));
            int[] focusModes = info.get(CameraCharacteristics.CONTROL_AF_AVAILABLE_MODES);
            boolean continuousFocus = focusModes != null && Arrays.stream(focusModes).anyMatch(mode -> mode == CaptureRequest.CONTROL_AF_MODE_CONTINUOUS_VIDEO);
            Integer orientation = info.get(CameraCharacteristics.SENSOR_ORIENTATION);
            int rotation = orientation == null ? 0 : orientation;
            reader = ImageReader.newInstance(size.getWidth(),size.getHeight(),ImageFormat.YUV_420_888,2);
            ImageReader source = reader;
            source.setOnImageAvailableListener(images -> {
                if (epoch != generation || !cameraWanted) return;
                try (Image image = images.acquireLatestImage()) {
                    if (image == null || SystemClock.elapsedRealtimeNanos()-lastFrame < 66_000_000L) return;
                    lastFrame = SystemClock.elapsedRealtimeNanos();
                    Image.Plane[] planes = image.getPlanes();
                    int turn = getSystemService(android.hardware.display.DisplayManager.class).getDisplay(android.view.Display.DEFAULT_DISPLAY).getRotation() * 90;
                    int oriented = (rotation + (facing == CameraCharacteristics.LENS_FACING_FRONT ? turn : -turn) + 360) % 360;
                    onCameraFrame(planes[0].getBuffer(),planes[1].getBuffer(),planes[2].getBuffer(),image.getWidth(),image.getHeight(),
                        planes[0].getRowStride(),planes[1].getRowStride(),planes[1].getPixelStride(),oriented);
                } catch (IllegalStateException ignored) { }
            },frames);
            manager.openCamera(selected,new CameraDevice.StateCallback() {
                @Override public void onOpened(CameraDevice opened) {
                    if (epoch != generation || !cameraWanted) {opened.close();return;}
                    camera = opened;
                    try {
                        request = opened.createCaptureRequest(CameraDevice.TEMPLATE_RECORD);
                        request.addTarget(source.getSurface());
                        request.set(CaptureRequest.CONTROL_AF_MODE,continuousFocus ? CaptureRequest.CONTROL_AF_MODE_CONTINUOUS_VIDEO : CaptureRequest.CONTROL_AF_MODE_OFF);
                        opened.createCaptureSession(Arrays.asList(source.getSurface()),new CameraCaptureSession.StateCallback() {
                            @Override public void onConfigured(CameraCaptureSession configured) {
                                if (epoch != generation || !cameraWanted) {configured.close();return;}
                                session = configured;
                                try {configured.setRepeatingRequest(request.build(),null,frames);
                                    onCameraStarted(rotation % 180 == 0 ? size.getWidth() : size.getHeight(),rotation % 180 == 0 ? size.getHeight() : size.getWidth());
                                } catch (CameraAccessException | RuntimeException error) {main.post(() -> {if (epoch == generation) cameraFailed();});}
                            }
                            @Override public void onConfigureFailed(CameraCaptureSession failed) {main.post(() -> {if (epoch == generation) cameraFailed();});}
                        },frames);
                    } catch (CameraAccessException | RuntimeException error) {main.post(() -> {if (epoch == generation) cameraFailed();});}
                }
                @Override public void onDisconnected(CameraDevice opened) {opened.close();main.post(() -> {if (epoch == generation) cameraFailed();});}
                @Override public void onError(CameraDevice opened,int error) {onDisconnected(opened);}
            },main);
        } catch (CameraAccessException | RuntimeException error) {cameraFailed();}
    }
    private void cameraFailed() {
        cameraWanted = false;closeCamera();
        if (microphoneActive) onStartCommand(new Intent(this,MeetingCaptureService.class).putExtra("action",14),0,0);
        else stopSelf();
    }
    private void closeCamera() {
        ++generation;torch = false;
        if (session != null) {session.close();session = null;}
        if (camera != null) {camera.close();camera = null;}
        ImageReader old = reader;reader = null;
        if (old != null && frames != null) frames.post(old::close);
        onCameraStopped();
    }
    @Override public void onDestroy() {
        running = false;cameraWanted = false;microphoneActive = false;closeCamera();onMicrophoneStopped();
        if (worker != null) worker.quitSafely();
        stopForeground(STOP_FOREGROUND_REMOVE);super.onDestroy();
    }
}
