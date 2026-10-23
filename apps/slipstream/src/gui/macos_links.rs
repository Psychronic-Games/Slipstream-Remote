//! macOS: `slipstream://` links (a station's "Connect this app", meeting
//! invitations) arrive as an Apple Event, which winit doesn't pass on. Catch
//! it ourselves and hand the link to the app through `single::inbox`, like
//! Android's intents. The scheme is declared in Info.plist (build-macos.sh).
use objc2::{
    AllocAnyThread, define_class, msg_send,
    rc::Retained,
    runtime::{AnyObject, NSObject},
    sel,
};
use std::ffi::{CStr, c_char};

/// Four-character Apple Event codes.
const fn code(c: &[u8; 4]) -> u32 {
    u32::from_be_bytes(*c)
}
/// kInternetEventClass and kAEGetURL are both 'GURL'.
const GET_URL: u32 = code(b"GURL");
/// keyDirectObject: the URL itself.
const DIRECT_OBJECT: u32 = code(b"----");

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "SlipstreamLinkHandler"]
    struct LinkHandler;

    impl LinkHandler {
        #[unsafe(method(handleGetURLEvent:withReplyEvent:))]
        fn handle_get_url(&self, event: &AnyObject, _reply: &AnyObject) {
            // SAFETY: the NSAppleEventDescriptor methods, as AppKit documents
            // them; UTF8String's buffer lives as long as the string.
            let url = unsafe {
                let param: Option<Retained<AnyObject>> = msg_send![event, paramDescriptorForKeyword: DIRECT_OBJECT];
                let Some(param) = param else { return };
                let text: Option<Retained<AnyObject>> = msg_send![&*param, stringValue];
                let Some(text) = text else { return };
                let utf8: *const c_char = msg_send![&*text, UTF8String];
                if utf8.is_null() {
                    return;
                }
                CStr::from_ptr(utf8).to_string_lossy().into_owned()
            };
            super::single::inbox::deliver(url);
        }
    }
);

/// Becomes the handler for 'GURL' events. Called before the event loop and
/// again once the window is up, as AppKit installs its own while launching.
pub fn install() {
    // SAFETY: a plain NSObject subclass; NSAppleEventManager's documented
    // setEventHandler:andSelector:forEventClass:andEventID:. The handler is
    // kept for the life of the process (the manager doesn't retain it).
    unsafe {
        let handler: Retained<LinkHandler> = msg_send![LinkHandler::alloc(), init];
        let manager: Option<Retained<AnyObject>> = msg_send![objc2::class!(NSAppleEventManager), sharedAppleEventManager];
        let Some(manager) = manager else { return };
        let _: () = msg_send![
            &*manager,
            setEventHandler: &*handler,
            andSelector: sel!(handleGetURLEvent:withReplyEvent:),
            forEventClass: GET_URL,
            andEventID: GET_URL
        ];
        std::mem::forget(handler);
    }
}
