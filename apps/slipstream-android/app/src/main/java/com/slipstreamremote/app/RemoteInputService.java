package com.slipstreamremote.app;

import android.accessibilityservice.*;
import android.content.Intent;
import android.graphics.Path;
import android.os.*;
import android.util.DisplayMetrics;
import android.view.KeyEvent;
import android.view.accessibility.*;

/** Receives input only through the authenticated Rust host and while sharing. */
public final class RemoteInputService extends AccessibilityService {
    static { System.loadLibrary("slipstream_android"); }
    private static volatile RemoteInputService current;
    private final Handler main = new Handler(Looper.getMainLooper());
    private float x = .5f, y = .5f, sentX, sentY;
    private boolean pressed, busy, shifted, caps, control;
    private int gestureGeneration;
    private GestureDescription.StrokeDescription stroke;
    private static native void onAvailability(boolean enabled);
    private static native boolean inputAllowed();
    static boolean available() { return current != null; }
    @Override public void onServiceConnected() {
        current = this;
        if (Build.VERSION.SDK_INT >= 33) {
            AccessibilityServiceInfo info = getServiceInfo();
            info.flags |= AccessibilityServiceInfo.FLAG_INPUT_METHOD_EDITOR;
            setServiceInfo(info);
        }
        onAvailability(true);
    }
    @Override public boolean onUnbind(Intent intent) { reset();current = null;onAvailability(false);return super.onUnbind(intent); }
    @Override public void onDestroy() { reset();current = null;onAvailability(false);super.onDestroy(); }
    @Override public void onAccessibilityEvent(AccessibilityEvent event) {}
    @Override public void onInterrupt() { reset(); }
    static void reset() {
        RemoteInputService service = current;
        if (service != null) service.main.post(service::release);
    }
    private void release() {
        pressed = false;shifted = false;control = false;gestureGeneration++;
        // End a continued touch even when capture was revoked.
        if (stroke != null && stroke.willContinue()) {
            Path path = new Path();path.moveTo(sentX,sentY);
            dispatchGesture(new GestureDescription.Builder().addStroke(stroke.continueStroke(path,0,1,false)).build(),null,null);
        }
        stroke = null;busy = false;
    }
    static void dispatch(int kind, float x, float y, String key, boolean down) {
        RemoteInputService service = current;
        if (service == null) return;
        service.main.post(() -> {
            if (kind == 5) {service.release();return;}
            if (!ScreenShareService.active || !inputAllowed()) {service.release();return;}
            service.input(kind,x,y,key,down);
        });
    }
    private void input(int kind, float a, float b, String key, boolean down) {
        switch (kind) {
            case 0: x = a;y = b;if (pressed) pump();break;
            case 1:
                if (down) {if (pressed) return;pressed = true;stroke = null;gestureGeneration++;}
                else pressed = false;
                pump();break;
            case 3:
                if (pressed || busy) return;
                DisplayMetrics m = metrics();
                float sx = Math.max(.15f, Math.min(.85f,x)) * m.widthPixels;
                float sy = Math.max(.25f, Math.min(.75f,y)) * m.heightPixels;
                Path path = new Path();path.moveTo(sx,sy);
                path.lineTo(Math.max(1,Math.min(m.widthPixels-1,sx+a*m.widthPixels*.10f)),
                    Math.max(1,Math.min(m.heightPixels-1,sy+b*m.heightPixels*.10f)));
                dispatchGesture(new GestureDescription.Builder().addStroke(new GestureDescription.StrokeDescription(path,0,180)).build(),null,null);break;
            case 4: key(key,down);break;
            case 6: if (down) {release();performGlobalAction(GLOBAL_ACTION_BACK);}break;
            case 7: if (down) {release();performGlobalAction(GLOBAL_ACTION_HOME);}break;
            case 8: if (down) {release();performGlobalAction(GLOBAL_ACTION_RECENTS);}break;
        }
    }
    private DisplayMetrics metrics() {
        DisplayMetrics m = new DisplayMetrics();
        getSystemService(android.hardware.display.DisplayManager.class).getDisplay(android.view.Display.DEFAULT_DISPLAY).getRealMetrics(m);
        return m;
    }
    /** Continued strokes provide real held touches, live drags and long presses. */
    private void pump() {
        if (busy || (!pressed && stroke == null)) return;
        if (!ScreenShareService.active || !inputAllowed()) {release();return;}
        DisplayMetrics metrics = metrics();
        float targetX = Math.max(1,Math.min(metrics.widthPixels-1,x*metrics.widthPixels));
        float targetY = Math.max(1,Math.min(metrics.heightPixels-1,y*metrics.heightPixels));
        // A stationary continued stroke with no up/down produces no events;
        // Android cancels it. Keep the held touch until movement or release.
        if (stroke != null && pressed && targetX == sentX && targetY == sentY) return;
        Path path = new Path();
        if (stroke == null) path.moveTo(targetX,targetY);
        else {path.moveTo(sentX,sentY);path.lineTo(targetX,targetY);}
        boolean continuing = pressed;
        GestureDescription.StrokeDescription next = stroke == null
            ? new GestureDescription.StrokeDescription(path,0,24,continuing)
            : stroke.continueStroke(path,0,24,continuing);
        final int generation = gestureGeneration;
        busy = true;stroke = next;sentX = targetX;sentY = targetY;
        boolean accepted = dispatchGesture(new GestureDescription.Builder().addStroke(next).build(),new GestureResultCallback() {
            @Override public void onCompleted(GestureDescription gesture) {
                if (generation != gestureGeneration) return;
                busy = false;stroke = continuing ? next : null;
                if (stroke != null) main.postDelayed(RemoteInputService.this::pump,8);
            }
            @Override public void onCancelled(GestureDescription gesture) {
                if (generation != gestureGeneration) return;
                busy = false;stroke = null;pressed = false;
            }
        },main);
        if (!accepted) {busy = false;stroke = null;pressed = false;}
    }
    private void key(String code, boolean down) {
        if (code.startsWith("Shift")) {shifted = down;return;}
        if (code.startsWith("Control")) {control = down;return;}
        if (!down) return;
        if ("CapsLock".equals(code)) {caps = !caps;return;}
        if ("Escape".equals(code) || "AndroidBack".equals(code)) {release();performGlobalAction(GLOBAL_ACTION_BACK);return;}
        if ("MetaLeft".equals(code) || "MetaRight".equals(code) || "AndroidHome".equals(code)) {release();performGlobalAction(GLOBAL_ACTION_HOME);return;}
        if ("AndroidRecents".equals(code)) {release();performGlobalAction(GLOBAL_ACTION_RECENTS);return;}
        String text = null;
        if (code.matches("Key[A-Z]")) text = (shifted ^ caps) ? code.substring(3) : code.substring(3).toLowerCase(java.util.Locale.ROOT);
        if (code.matches("Digit[0-9]")) {int digit = code.charAt(5)-'0';text = String.valueOf(shifted ? ")!@#$%^&*(".charAt(digit) : code.charAt(5));}
        String[] keys = {"Space","Minus","Equal","BracketLeft","BracketRight","Backslash","Semicolon","Quote","Comma","Period","Slash","Backquote"};
        String[] lower = {" ","-","=","[","]","\\",";","'",",",".","/","`"};
        String[] upper = {" ","_","+","{","}","|",":","\"","<",">","?","~"};
        for (int i=0;i<keys.length;i++) if (keys[i].equals(code)) text = shifted ? upper[i] : lower[i];
        if (Build.VERSION.SDK_INT >= 33 && getInputMethod() != null && getInputMethod().getCurrentInputConnection() != null) {
            InputMethod.AccessibilityInputConnection connection = getInputMethod().getCurrentInputConnection();
            if (control) {
                if ("KeyA".equals(code)) connection.performContextMenuAction(android.R.id.selectAll);
                return;
            }
            if (text != null) {connection.commitText(text,1,null);return;}
            int keyCode = androidKey(code);
            if (keyCode != KeyEvent.KEYCODE_UNKNOWN) {
                connection.sendKeyEvent(new KeyEvent(KeyEvent.ACTION_DOWN,keyCode));
                connection.sendKeyEvent(new KeyEvent(KeyEvent.ACTION_UP,keyCode));
            }
            return;
        }
        // API 26–32: ordinary editable fields through accessibility actions.
        AccessibilityNodeInfo root = getRootInActiveWindow();
        if (root == null) return;
        AccessibilityNodeInfo node = root.findFocus(AccessibilityNodeInfo.FOCUS_INPUT);
        if (node == null || !node.isEditable()) return;
        String old = node.getText() == null ? "" : node.getText().toString();
        int start = Math.max(0,Math.min(old.length(),node.getTextSelectionStart()));
        int end = Math.max(start,Math.min(old.length(),node.getTextSelectionEnd()));
        Bundle args = new Bundle();
        if (control) {
            if ("KeyA".equals(code)) {
                args.putInt(AccessibilityNodeInfo.ACTION_ARGUMENT_SELECTION_START_INT,0);
                args.putInt(AccessibilityNodeInfo.ACTION_ARGUMENT_SELECTION_END_INT,old.length());
                node.performAction(AccessibilityNodeInfo.ACTION_SET_SELECTION,args);
            }
            return;
        }
        if ("Backspace".equals(code)) {if (start == end && start > 0) start = old.offsetByCodePoints(start,-1);text = "";}
        if ("Delete".equals(code)) {if (start == end && end < old.length()) end = old.offsetByCodePoints(end,1);text = "";}
        if ("Enter".equals(code)) text = "\n";
        if (text == null) return;
        args.putCharSequence(AccessibilityNodeInfo.ACTION_ARGUMENT_SET_TEXT_CHARSEQUENCE,old.substring(0,start)+text+old.substring(end));
        node.performAction(AccessibilityNodeInfo.ACTION_SET_TEXT,args);
        args.clear();args.putInt(AccessibilityNodeInfo.ACTION_ARGUMENT_SELECTION_START_INT,start+text.length());
        args.putInt(AccessibilityNodeInfo.ACTION_ARGUMENT_SELECTION_END_INT,start+text.length());
        node.performAction(AccessibilityNodeInfo.ACTION_SET_SELECTION,args);
    }
    private static int androidKey(String code) {
        switch (code) {
            case "Backspace": return KeyEvent.KEYCODE_DEL;
            case "Delete": return KeyEvent.KEYCODE_FORWARD_DEL;
            case "Enter": return KeyEvent.KEYCODE_ENTER;
            case "Tab": return KeyEvent.KEYCODE_TAB;
            case "ArrowLeft": return KeyEvent.KEYCODE_DPAD_LEFT;
            case "ArrowRight": return KeyEvent.KEYCODE_DPAD_RIGHT;
            case "ArrowUp": return KeyEvent.KEYCODE_DPAD_UP;
            case "ArrowDown": return KeyEvent.KEYCODE_DPAD_DOWN;
            case "Home": return KeyEvent.KEYCODE_MOVE_HOME;
            case "End": return KeyEvent.KEYCODE_MOVE_END;
            default: return KeyEvent.KEYCODE_UNKNOWN;
        }
    }
}
