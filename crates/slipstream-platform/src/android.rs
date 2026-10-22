//! Android MediaProjection capture and explicitly enabled accessibility input.
use crate::{Backend, FrameStream, android_frames::CaptureHub};
use anyhow::{Context, Result, bail};
use jni::{
    JavaVM,
    objects::{GlobalRef, JClass, JObject, JValue},
};
use slipstream_core::proto::{DisplayInfo, InputEvent, MouseButton};
use std::sync::{
    OnceLock,
    atomic::{AtomicBool, Ordering},
};

struct Bridge {
    vm: JavaVM,
    class: GlobalRef,
}
static BRIDGE: OnceLock<Bridge> = OnceLock::new();
static HUB: OnceLock<CaptureHub> = OnceLock::new();
static CAMERA: OnceLock<CaptureHub> = OnceLock::new();
fn camera() -> &'static CaptureHub {
    CAMERA.get_or_init(CaptureHub::default)
}
pub fn camera_dimensions() -> Option<(u32, u32)> {
    camera().dimensions()
}
pub fn camera_stream() -> Result<FrameStream> {
    camera().subscribe()
}
pub fn camera_started(width: u32, height: u32) {
    camera().start(width, height);
}
pub fn camera_stopped() {
    camera().stop("Phone camera sharing stopped");
}
static CAMERA_EPOCH: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub fn camera_start(index: u32) -> Result<u64> {
    let epoch = CAMERA_EPOCH.fetch_add(1, Ordering::AcqRel) + 1;
    camera_stopped();
    action(if index == 1 { 11 } else { 10 })?;
    Ok(epoch)
}
pub fn camera_stop(epoch: u64) {
    if CAMERA_EPOCH
        .compare_exchange(epoch, epoch + 1, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
    {
        camera_stopped();
        let _ = action(12);
    }
}
pub fn camera_frame(data: &[u8], width: u32, height: u32) {
    let _ = camera().publish(data, width, height, width as usize * 4, 4);
}
pub fn microphone_ready() -> bool {
    let Some(b) = BRIDGE.get() else {
        return false;
    };
    let Ok(mut env) = b.vm.attach_current_thread() else {
        return false;
    };
    let Ok(class) = env.new_local_ref(b.class.as_obj()).map(JClass::from) else {
        return false;
    };
    env.call_static_method(class, "microphoneReady", "()Z", &[])
        .and_then(|v| v.z())
        .unwrap_or(false)
}
type CallHandler = Box<dyn Fn(u64, bool) + Send + Sync>;
static CALL_HANDLER: OnceLock<CallHandler> = OnceLock::new();
pub fn install_call_handler(handler: impl Fn(u64, bool) + Send + Sync + 'static) {
    let _ = CALL_HANDLER.set(Box::new(handler));
}
pub fn answer_call(id: u64, accepted: bool) {
    if let Some(handler) = CALL_HANDLER.get() {
        handler(id, accepted);
    }
}
pub fn notify_call(name: Option<&str>, id: u64) {
    let Some(b) = BRIDGE.get() else {
        return;
    };
    let Ok(mut env) = b.vm.attach_current_thread() else {
        return;
    };
    let Ok(class) = env.new_local_ref(b.class.as_obj()).map(JClass::from) else {
        return;
    };
    let Ok(name) = env.new_string(name.unwrap_or("")) else {
        return;
    };
    if env
        .call_static_method(
            class,
            "meetingCall",
            "(Ljava/lang/String;J)V",
            &[JValue::Object(&name), JValue::Long(id as i64)],
        )
        .is_err()
    {
        let _ = env.exception_clear();
    }
}
static ACCESS: AtomicBool = AtomicBool::new(false);
static FULL_DISPLAY: AtomicBool = AtomicBool::new(true);
static MESSAGE: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());
fn hub() -> &'static CaptureHub {
    HUB.get_or_init(CaptureHub::default)
}

pub fn install(vm: JavaVM, class: GlobalRef) {
    let _ = BRIDGE.set(Bridge { vm, class });
}
pub fn device_name() -> Result<String> {
    let b = BRIDGE.get().context("Android bridge not initialized")?;
    let mut env = b.vm.attach_current_thread()?;
    let class = JClass::from(env.new_local_ref(b.class.as_obj())?);
    let name = env
        .call_static_method(class, "deviceName", "()Ljava/lang/String;", &[])?
        .l()?;
    Ok(env.get_string(&jni::objects::JString::from(name))?.into())
}
pub fn action(action: i32) -> Result<()> {
    let b = BRIDGE.get().context("Android bridge not initialized")?;
    let mut env = b.vm.attach_current_thread()?;
    let class = JClass::from(env.new_local_ref(b.class.as_obj())?);
    let result = env.call_static_method(class, "hostAction", "(I)V", &[JValue::Int(action)]);
    if result.is_err() {
        let _ = env.exception_clear();
    }
    result?;
    Ok(())
}
pub fn status() -> (bool, bool, String) {
    (
        hub().dimensions().is_some(),
        ACCESS.load(Ordering::Acquire),
        MESSAGE.lock().unwrap().clone(),
    )
}
pub fn started(width: u32, height: u32, full: bool) {
    FULL_DISPLAY.store(full, Ordering::Release);
    hub().start(width, height);
    *MESSAGE.lock().unwrap() =
        "Screen sharing active. Stop sharing here or in Android's notification.".into();
}
pub fn stopped(message: String) {
    hub().stop(&message);
    *MESSAGE.lock().unwrap() = message;
}
pub fn set_accessibility(enabled: bool) {
    ACCESS.store(enabled, Ordering::Release);
}
pub fn set_full_display(full: bool) {
    FULL_DISPLAY.store(full, Ordering::Release);
}
pub fn input_allowed() -> bool {
    ACCESS.load(Ordering::Acquire)
        && FULL_DISPLAY.load(Ordering::Acquire)
        && hub().has_viewers()
        && hub().dimensions().is_some()
}
pub fn frame(data: &[u8], width: u32, height: u32, row: usize, pixel: usize) {
    if let Err(e) = hub().publish(data, width, height, row, pixel) {
        tracing::warn!("Android capture frame rejected: {e:#}");
    }
}

pub struct AndroidBackend;
impl AndroidBackend {
    pub fn open() -> Result<Self> {
        if hub().dimensions().is_none() {
            bail!("On the phone, tap Start sharing and approve Android's screen-sharing prompt");
        }
        Ok(Self)
    }
}
impl Backend for AndroidBackend {
    fn name(&self) -> &'static str {
        "android-mediaprojection"
    }
    fn displays(&self) -> Vec<DisplayInfo> {
        hub()
            .dimensions()
            .map(|(width, height)| {
                vec![DisplayInfo {
                    index: 0,
                    name: "Phone screen".into(),
                    width,
                    height,
                    x: 0,
                    y: 0,
                    primary: true,
                }]
            })
            .unwrap_or_default()
    }
    fn input_supported(&self) -> bool {
        ACCESS.load(Ordering::Acquire) && FULL_DISPLAY.load(Ordering::Acquire)
    }
    fn capture(&mut self, display: u32, _fps: u32) -> Result<FrameStream> {
        if display != 0 {
            bail!("Phone has one shared screen");
        }
        Ok(hub().subscribe()?.with_cleanup(|| {
            let _ = dispatch(5, 0.0, 0.0, "", false);
        }))
    }
    fn inject(&mut self, _display: u32, event: InputEvent) -> Result<()> {
        if matches!(event, InputEvent::ReleaseAll) {
            return dispatch(5, 0.0, 0.0, "", false);
        }
        if !input_allowed() {
            bail!(
                "Enable Slipstream Remote in Android Accessibility settings and share the entire screen"
            );
        }
        let (kind, x, y, key, down) = match event {
            InputEvent::MouseMove { x, y } if x.is_finite() && y.is_finite() => (
                0,
                x.clamp(0.0, 1.0),
                y.clamp(0.0, 1.0),
                String::new(),
                false,
            ),
            InputEvent::MouseButton { button, down } => {
                let kind = match button {
                    MouseButton::Left => 1,
                    MouseButton::Right | MouseButton::Back => 6,
                    MouseButton::Middle => 7,
                    MouseButton::Forward => 8,
                };
                (kind, 0.0, 0.0, String::new(), down)
            }
            InputEvent::Wheel { dx, dy } if dx.is_finite() && dy.is_finite() => (
                3,
                dx.clamp(-8.0, 8.0),
                dy.clamp(-8.0, 8.0),
                String::new(),
                false,
            ),
            InputEvent::Key { code, down } if code.len() <= 64 => (4, 0.0, 0.0, code, down),
            _ => return Ok(()),
        };
        dispatch(kind, x, y, &key, down)
    }
}
fn dispatch(kind: i32, x: f32, y: f32, key: &str, down: bool) -> Result<()> {
    let b = BRIDGE.get().context("Android bridge not initialized")?;
    let mut env = b.vm.attach_current_thread()?;
    let class = JClass::from(env.new_local_ref(b.class.as_obj())?);
    let text = JObject::from(env.new_string(key)?);
    let result = env.call_static_method(
        class,
        "remoteInput",
        "(IFFLjava/lang/String;Z)V",
        &[
            JValue::Int(kind),
            JValue::Float(x),
            JValue::Float(y),
            JValue::Object(&text),
            JValue::Bool(u8::from(down)),
        ],
    );
    if result.is_err() {
        let _ = env.exception_clear();
    }
    result?;
    Ok(())
}
