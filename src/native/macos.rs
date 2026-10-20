//! macOS system-wide expansion using a CoreGraphics event tap.
//!
//! Safety/privacy design:
//! * Off by default; started only by an explicit user action in the app.
//! * Requires that Accessibility permission is *already* granted (`AXIsProcessTrusted`);
//!   we never prompt programmatically or modify TCC settings.
//! * Typed characters only go into the engine's bounded in-memory buffer (64 chars).
//!   Nothing typed is logged, written to disk, or sent anywhere.
//! * Secure keyboard entry (password fields) is detected and skipped.
//! * Our own synthetic events carry a marker (and our PID as source) and are ignored.
//! * The tap passes every event through unchanged; replacement is done by posting
//!   Backspace + text events afterwards from a separate injector thread.
//!
//! Injection is guarded (fail closed):
//! * Each match carries a *generation*. Any later real key press, mouse click, navigation
//!   key, shortcut, pause, stop, tap timeout or secure-input detection bumps the
//!   generation, which cancels pending/in-progress injection.
//! * The focused application (PID) and focused UI element are captured via Accessibility
//!   when the match is queued, and must be unchanged immediately before injection and
//!   before every injection step. If the focused app cannot be identified, nothing is
//!   injected. The key event's target PID (when macOS provides it) must equal the focused app.
//! * Gates (stop/pause/generation/secure input/exclusion) are re-checked before every
//!   synthetic key.
//!
//! App exclusions (by bundle identifier) apply twice: key events whose target app is
//! excluded are neither recorded nor matched, and injection is refused if the focused app is
//! excluded. Switching apps resets matching.
//!
//! Quick Search insertion (manual) reuses the same injector: the focused app and element are
//! captured *before* the panel opens; after it closes, the injector waits (bounded) for that
//! exact target to be focused again and then applies the same gates. Nothing typed is erased.

use super::{
    auto_skip, check_manual_insert, classify_key, enclosing_app_bundles, evaluate_gates,
    is_excluded, key_category, plan_injection, AbortReason, AutoSkip, CapturedTarget, GateSnapshot,
    InjectStep, KeyCategory, KeyContext, KeyInput, ManualInsertState, NativeDiagnostics,
    NativeState, NativeStats, Permissions, SYNTHETIC_MARKER,
};
use crate::engine::{Engine, Expansion};
use crate::model::{Library, Snippet};
use crate::template::{uses_clipboard, RenderContext};
use std::collections::{HashMap, HashSet};
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

type CFTypeRef = *const c_void;
type CFMachPortRef = *mut c_void;
type CFRunLoopRef = *mut c_void;
type CFRunLoopSourceRef = *mut c_void;
type CFStringRef = *const c_void;
type CGEventRef = *mut c_void;
type CGEventSourceRef = *mut c_void;
type CGEventTapProxy = *mut c_void;
type AXUIElementRef = *const c_void;

type TapCallback = extern "C" fn(CGEventTapProxy, u32, CGEventRef, *mut c_void) -> CGEventRef;

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    static kCFRunLoopCommonModes: CFStringRef;
    static kCFRunLoopDefaultMode: CFStringRef;
    fn CFMachPortCreateRunLoopSource(
        allocator: CFTypeRef,
        port: CFMachPortRef,
        order: isize,
    ) -> CFRunLoopSourceRef;
    fn CFMachPortInvalidate(port: CFMachPortRef);
    fn CFRunLoopGetCurrent() -> CFRunLoopRef;
    fn CFRunLoopAddSource(rl: CFRunLoopRef, source: CFRunLoopSourceRef, mode: CFStringRef);
    fn CFRunLoopRunInMode(mode: CFStringRef, seconds: f64, return_after_source: u8) -> i32;
    fn CFRunLoopStop(rl: CFRunLoopRef);
    fn CFRelease(cf: CFTypeRef);
    fn CFEqual(a: CFTypeRef, b: CFTypeRef) -> u8;
    fn CFStringCreateWithBytes(
        alloc: CFTypeRef,
        bytes: *const u8,
        len: isize,
        encoding: u32,
        is_external: u8,
    ) -> CFStringRef;
    fn CFStringGetCString(s: CFStringRef, buf: *mut u8, size: isize, encoding: u32) -> u8;
    fn CFURLCreateFromFileSystemRepresentation(
        alloc: CFTypeRef,
        buf: *const u8,
        len: isize,
        is_directory: u8,
    ) -> CFTypeRef;
    fn CFBundleCreate(alloc: CFTypeRef, url: CFTypeRef) -> CFTypeRef;
    fn CFBundleGetIdentifier(bundle: CFTypeRef) -> CFStringRef;
}

extern "C" {
    /// libproc (part of libSystem): executable path of a process.
    fn proc_pidpath(pid: i32, buffer: *mut c_void, size: u32) -> i32;
}

const PROC_PIDPATHINFO_MAXSIZE: usize = 4096;
/// Bounded cache of PID → (executable path, enclosing bundle identifiers).
const BUNDLE_CACHE_CAPACITY: usize = 128;
/// How long a Quick Search insertion waits for its original target to be focused again.
const MANUAL_REFOCUS_TIMEOUT: Duration = Duration::from_millis(1500);

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGEventTapCreate(
        tap: u32,
        place: u32,
        options: u32,
        events_of_interest: u64,
        callback: TapCallback,
        user_info: *mut c_void,
    ) -> CFMachPortRef;
    fn CGEventTapEnable(tap: CFMachPortRef, enable: bool);
    fn CGEventTapIsEnabled(tap: CFMachPortRef) -> bool;
    fn CGEventGetIntegerValueField(event: CGEventRef, field: u32) -> i64;
    fn CGEventSetIntegerValueField(event: CGEventRef, field: u32, value: i64);
    fn CGEventGetFlags(event: CGEventRef) -> u64;
    fn CGEventSetFlags(event: CGEventRef, flags: u64);
    fn CGEventKeyboardGetUnicodeString(
        event: CGEventRef,
        max_len: usize,
        actual_len: *mut usize,
        buf: *mut u16,
    );
    fn CGEventKeyboardSetUnicodeString(event: CGEventRef, len: usize, buf: *const u16);
    fn CGEventCreateKeyboardEvent(
        source: CGEventSourceRef,
        keycode: u16,
        key_down: bool,
    ) -> CGEventRef;
    fn CGEventSourceCreate(state: i32) -> CGEventSourceRef;
    fn CGEventPostToPid(pid: i32, event: CGEventRef);
    #[cfg(test)]
    fn CGEventGetType(event: CGEventRef) -> u32;
    fn CGEventSourceSetUserData(source: CGEventSourceRef, user_data: i64);
    #[cfg(test)]
    fn CGEventSourceGetUserData(source: CGEventSourceRef) -> i64;
    fn CGPreflightListenEventAccess() -> bool;
    fn CGPreflightPostEventAccess() -> bool;
}

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXIsProcessTrusted() -> u8;
    fn AXUIElementCreateSystemWide() -> AXUIElementRef;
    fn AXUIElementCopyAttributeValue(
        element: AXUIElementRef,
        attribute: CFStringRef,
        value: *mut CFTypeRef,
    ) -> i32;
    fn AXUIElementGetPid(element: AXUIElementRef, pid: *mut i32) -> i32;
    fn AXUIElementSetMessagingTimeout(element: AXUIElementRef, seconds: f32) -> i32;
}

#[link(name = "Carbon", kind = "framework")]
extern "C" {
    fn IsSecureEventInputEnabled() -> u8;
    static kTISPropertyUnicodeKeyLayoutData: CFStringRef;
    fn TISCopyCurrentKeyboardLayoutInputSource() -> CFTypeRef;
    fn TISGetInputSourceProperty(source: CFTypeRef, key: CFStringRef) -> CFTypeRef;
    fn LMGetKbdType() -> u8;
}

#[link(name = "CoreServices", kind = "framework")]
extern "C" {
    #[allow(clippy::too_many_arguments)]
    fn UCKeyTranslate(
        layout: *const c_void,
        virtual_key_code: u16,
        key_action: u16,
        modifier_key_state: u32,
        keyboard_type: u32,
        options: u32,
        dead_key_state: *mut u32,
        max_len: usize,
        actual_len: *mut usize,
        unicode: *mut u16,
    ) -> i32;
    fn CFDataGetBytePtr(data: CFTypeRef) -> *const u8;
    fn CFDataGetLength(data: CFTypeRef) -> isize;
}

// CGEventTapLocation / placement / options.
const SESSION_EVENT_TAP: u32 = 1;
const HEAD_INSERT: u32 = 0;
const TAP_OPTION_DEFAULT: u32 = 0;
const TAP_OPTION_LISTEN_ONLY: u32 = 1;
// CGEventType.
const EV_LEFT_MOUSE_DOWN: u32 = 1;
const EV_RIGHT_MOUSE_DOWN: u32 = 3;
const EV_KEY_DOWN: u32 = 10;
const EV_OTHER_MOUSE_DOWN: u32 = 25;
const EV_TAP_DISABLED_BY_TIMEOUT: u32 = 0xFFFF_FFFE;
const EV_TAP_DISABLED_BY_USER_INPUT: u32 = 0xFFFF_FFFF;
// CGEventField.
const FIELD_KEYCODE: u32 = 9;
const FIELD_TARGET_UNIX_PID: u32 = 40;
const FIELD_SOURCE_UNIX_PID: u32 = 41;
const FIELD_SOURCE_USER_DATA: u32 = 42;
const FIELD_SOURCE_STATE_ID: u32 = 45;
// CGEventSourceStateID.
const SOURCE_STATE_PRIVATE: i32 = -1;
const CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;

/// Delay that lets the triggering key reach the focused app before we edit around it.
/// Upper bound when reading a key event's Unicode string.
const MAX_EVENT_UTF16: usize = 64;
const SETTLE_DELAY: Duration = Duration::from_millis(30);
/// Extra settle time after the Quick Search panel closed and the target is focused again.
const MANUAL_SETTLE_DELAY: Duration = Duration::from_millis(80);
const START_TIMEOUT: Duration = Duration::from_secs(3);
/// Explicit bound on queued injection jobs.
const INJECT_QUEUE_CAPACITY: usize = 1;

extern "C" {
    fn pthread_main_np() -> i32;
}

/// True on the process main thread. TextInputSources (keyboard layout) must only be used
/// there, so `start` and `refresh_keyboard_layout` enforce it.
pub fn is_main_thread() -> bool {
    // SAFETY: pure query.
    unsafe { pthread_main_np() == 1 }
}

pub fn permissions() -> Permissions {
    // SAFETY: read-only system queries without prompts.
    unsafe {
        Permissions {
            supported: true,
            accessibility: AXIsProcessTrusted() != 0,
            listen_events: CGPreflightListenEventAccess(),
            post_events: CGPreflightPostEventAccess(),
            secure_input: IsSecureEventInputEnabled() != 0,
        }
    }
}

fn secure_input_active() -> bool {
    // SAFETY: read-only query.
    unsafe { IsSecureEventInputEnabled() != 0 }
}

/// Opens System Settings at Privacy & Security → Accessibility. Does not change anything.
pub fn open_accessibility_settings() -> Result<(), String> {
    std::process::Command::new("/usr/bin/open")
        .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility")
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

// ---- Keyboard layout fallback ----------------------------------------------------------

/// Copy of the current keyboard layout's `uchr` data, taken on the main thread.
///
/// Some synthetic key events (e.g. created from a virtual key code only) reach the tap with
/// an empty Unicode string. Real keyboards always carry one. For those events we translate
/// the key code with this snapshot instead of treating the key as a reset.
pub struct LayoutSnapshot {
    /// 8-byte aligned storage for the `UCKeyboardLayout` bytes.
    data: Vec<u64>,
    kbd_type: u32,
}

/// Snapshot the current keyboard layout. **Main thread only** (TextInputSources is not
/// thread safe). Returns `None` for layouts without `uchr` data.
pub fn current_layout_snapshot() -> Option<LayoutSnapshot> {
    // SAFETY: documented TIS calls on the main thread; the source is released, the
    // property is borrowed (Get rule) and copied before release.
    unsafe {
        let source = TISCopyCurrentKeyboardLayoutInputSource();
        if source.is_null() {
            return None;
        }
        let data = TISGetInputSourceProperty(source, kTISPropertyUnicodeKeyLayoutData);
        let result = if data.is_null() {
            None
        } else {
            let len = CFDataGetLength(data).max(0) as usize;
            let ptr = CFDataGetBytePtr(data);
            if len == 0 || ptr.is_null() {
                None
            } else {
                let mut buf = vec![0u64; len.div_ceil(8)];
                std::ptr::copy_nonoverlapping(ptr, buf.as_mut_ptr() as *mut u8, len);
                Some(LayoutSnapshot {
                    data: buf,
                    kbd_type: LMGetKbdType() as u32,
                })
            }
        };
        CFRelease(source);
        result
    }
}

/// CGEventFlags → Carbon modifier byte expected by `UCKeyTranslate`.
pub fn carbon_modifier_state(flags: u64) -> u32 {
    let mut carbon: u32 = 0;
    if flags & 0x0010_0000 != 0 {
        carbon |= 1 << 8; // cmdKey
    }
    if flags & 0x0002_0000 != 0 {
        carbon |= 1 << 9; // shiftKey
    }
    if flags & 0x0001_0000 != 0 {
        carbon |= 1 << 10; // alphaLock
    }
    if flags & 0x0008_0000 != 0 {
        carbon |= 1 << 11; // optionKey
    }
    if flags & 0x0004_0000 != 0 {
        carbon |= 1 << 12; // controlKey
    }
    (carbon >> 8) & 0xFF
}

/// Translate a virtual key code with a layout snapshot. Dead keys are not composed.
/// Pure with respect to process state; safe to call from the tap thread.
pub fn translate_keycode(layout: &LayoutSnapshot, keycode: u16, flags: u64) -> String {
    let mut dead: u32 = 0;
    let mut buf = [0u16; 8];
    let mut len: usize = 0;
    // SAFETY: layout bytes are a copy of valid `uchr` data; output buffer is sized.
    let status = unsafe {
        UCKeyTranslate(
            layout.data.as_ptr() as *const c_void,
            keycode,
            0, // kUCKeyActionDown
            carbon_modifier_state(flags),
            layout.kbd_type,
            1, // kUCKeyTranslateNoDeadKeysMask
            &mut dead,
            buf.len(),
            &mut len,
            buf.as_mut_ptr(),
        )
    };
    if status != 0 {
        return String::new();
    }
    String::from_utf16_lossy(&buf[..len.min(buf.len())])
}

// ---- Bundle identifiers of processes ---------------------------------------------------

fn executable_path(pid: i32) -> Option<String> {
    if pid <= 0 {
        return None;
    }
    let mut buf = vec![0u8; PROC_PIDPATHINFO_MAXSIZE];
    // SAFETY: buffer is valid for `len` bytes; returns the number of bytes written.
    let n = unsafe { proc_pidpath(pid, buf.as_mut_ptr() as *mut c_void, buf.len() as u32) };
    if n <= 0 {
        return None;
    }
    buf.truncate(n as usize);
    String::from_utf8(buf).ok()
}

/// `CFBundleIdentifier` of the bundle at `path`, read through CoreFoundation.
fn bundle_identifier(path: &str) -> Option<String> {
    // SAFETY: CF objects created here are released; the identifier follows the Get rule
    // (owned by the bundle) and is copied before the bundle is released.
    unsafe {
        let url = CFURLCreateFromFileSystemRepresentation(
            std::ptr::null(),
            path.as_ptr(),
            path.len() as isize,
            1,
        );
        if url.is_null() {
            return None;
        }
        let bundle = CFBundleCreate(std::ptr::null(), url);
        CFRelease(url);
        if bundle.is_null() {
            return None;
        }
        let id = CFBundleGetIdentifier(bundle);
        let mut out = None;
        if !id.is_null() {
            let mut buf = [0u8; 512];
            if CFStringGetCString(
                id,
                buf.as_mut_ptr(),
                buf.len() as isize,
                CF_STRING_ENCODING_UTF8,
            ) != 0
            {
                let len = buf.iter().position(|b| *b == 0).unwrap_or(0);
                out = String::from_utf8(buf[..len].to_vec()).ok();
            }
        }
        CFRelease(bundle);
        out.filter(|s| !s.is_empty())
    }
}

/// Bundle identifiers of the `.app` bundles containing a process's executable, innermost
/// first. Cached per PID; the executable path is re-read every time so a reused PID is
/// never confused with the earlier process.
fn bundle_ids_for_pid(cache: &Mutex<HashMap<i32, (String, Vec<String>)>>, pid: i32) -> Vec<String> {
    let Some(path) = executable_path(pid) else {
        return Vec::new();
    };
    if let Ok(c) = cache.lock() {
        if let Some((p, ids)) = c.get(&pid) {
            if *p == path {
                return ids.clone();
            }
        }
    }
    let ids: Vec<String> = enclosing_app_bundles(&path)
        .iter()
        .filter_map(|b| bundle_identifier(b))
        .collect();
    if let Ok(mut c) = cache.lock() {
        if c.len() >= BUNDLE_CACHE_CAPACITY {
            c.clear();
        }
        c.insert(pid, (path, ids.clone()));
    }
    ids
}

// ---- Focus identity via Accessibility -------------------------------------------------

/// Focused application PID plus (when exposed) the focused UI element.
struct FocusIdentity {
    pid: i32,
    /// Retained AXUIElementRef or null if the app does not expose a focused element.
    element: CFTypeRef,
}

// SAFETY: the element is a retained AXUIElementRef (a CFType). CF retain/release are
// thread-safe and the AX API may be called from any thread; the identity is only moved
// between threads, never used concurrently.
unsafe impl Send for FocusIdentity {}

// None means no element; Some(None) means an exposed element with an unreadable PID.
// Never turn an invalid or inconsistent element into an app-only identity.
fn focused_owner(element_pid: Option<Option<i32>>, app_pid: Option<i32>) -> Option<i32> {
    match element_pid {
        Some(pid) => pid.filter(|pid| *pid > 0 && app_pid.is_none_or(|app| app == *pid)),
        None => app_pid.filter(|pid| *pid > 0),
    }
}

unsafe fn ax_pid(element: AXUIElementRef) -> Option<i32> {
    let mut pid = 0;
    // SAFETY: caller supplies a live AX element; pid points to writable storage.
    (AXUIElementGetPid(element, &mut pid) == 0 && pid > 0).then_some(pid)
}

// AX messaging timeouts are process-wide when set on the system object. Keep all
// focus queries short, including injector queries, so they cannot lengthen tap queries.
const FOCUS_QUERY_TIMEOUT: f32 = 0.005;

fn keyboard_focus_pid() -> Option<i32> {
    // SAFETY: all copied/created CF objects are released; no field contents are read.
    unsafe {
        let system = AXUIElementCreateSystemWide();
        if system.is_null() {
            return None;
        }
        if AXUIElementSetMessagingTimeout(system, FOCUS_QUERY_TIMEOUT) != 0 {
            CFRelease(system);
            return None;
        }
        // The tap uses just one bounded attribute query, without app-level fallback.
        let element = copy_ax_attribute(system, "AXFocusedUIElement");
        let element = match element {
            Ok(element) => element,
            Err(()) => {
                CFRelease(system);
                return None;
            }
        };
        let pid = if element.is_null() {
            None
        } else {
            let pid = ax_pid(element);
            CFRelease(element);
            pid
        };
        CFRelease(system);
        pid
    }
}

impl FocusIdentity {
    fn capture() -> Option<Self> {
        // SAFETY: standard AX calls; every created/copied object is released below.
        unsafe {
            let system = AXUIElementCreateSystemWide();
            if system.is_null() {
                return None;
            }
            if AXUIElementSetMessagingTimeout(system, FOCUS_QUERY_TIMEOUT) != 0 {
                CFRelease(system);
                return None;
            }
            let result = (|| {
                // A global query error is not evidence that no element exists. Never
                // fall back to the active app behind a panel after an IPC failure.
                let mut element = copy_ax_attribute(system, "AXFocusedUIElement").ok()?;
                let mut app_pid = None;
                if element.is_null() {
                    let app = copy_ax_attribute(system, "AXFocusedApplication").ok()?;
                    if !app.is_null() {
                        let fallback = (|| {
                            if AXUIElementSetMessagingTimeout(app, FOCUS_QUERY_TIMEOUT) != 0 {
                                return None;
                            }
                            let pid = ax_pid(app)?;
                            let element = copy_ax_attribute(app, "AXFocusedUIElement").ok()?;
                            Some((pid, element))
                        })();
                        CFRelease(app);
                        let (pid, focused) = fallback?;
                        app_pid = Some(pid);
                        element = focused;
                    }
                }
                let element_pid = (!element.is_null()).then(|| ax_pid(element));
                let pid = focused_owner(element_pid, app_pid);
                let result = pid.map(|pid| FocusIdentity { pid, element });
                if result.is_none() && !element.is_null() {
                    CFRelease(element);
                }
                result
            })();
            CFRelease(system);
            result
        }
    }

    /// Same app, and same focused element when one was known at capture time.
    fn matches(&self, now: &FocusIdentity) -> bool {
        if self.pid != now.pid {
            return false;
        }
        if self.element.is_null() {
            return true;
        }
        // SAFETY: both are valid retained CF objects.
        !now.element.is_null() && unsafe { CFEqual(self.element, now.element) } != 0
    }
}

impl Drop for FocusIdentity {
    fn drop(&mut self) {
        if !self.element.is_null() {
            // SAFETY: we own one retain on element.
            unsafe { CFRelease(self.element) };
        }
    }
}

// AXError.h: only unsupported/noValue mean an absent attribute. In particular,
// cannotComplete (-25204, including IPC timeouts) must not enable app fallback.
fn ax_attribute_present(status: i32) -> Result<bool, ()> {
    match status {
        0 => Ok(true),
        -25205 | -25212 => Ok(false),
        _ => Err(()),
    }
}

/// Copy an AX attribute; null means absent, Err means querying failed.
unsafe fn copy_ax_attribute(element: AXUIElementRef, name: &str) -> Result<CFTypeRef, ()> {
    let attr = CFStringCreateWithBytes(
        std::ptr::null(),
        name.as_ptr(),
        name.len() as isize,
        CF_STRING_ENCODING_UTF8,
        0,
    );
    if attr.is_null() {
        return Err(());
    }
    let mut value: CFTypeRef = std::ptr::null();
    let err = AXUIElementCopyAttributeValue(element, attr, &mut value);
    CFRelease(attr);
    match ax_attribute_present(err) {
        Ok(true) if !value.is_null() => Ok(value),
        status => {
            if !value.is_null() {
                CFRelease(value);
            }
            match status {
                Ok(false) => Ok(std::ptr::null()),
                _ => Err(()),
            }
        }
    }
}

// ---- Shared state ---------------------------------------------------------------------

struct Shared {
    engine: Mutex<Engine>,
    paused: AtomicBool,
    shutdown: AtomicBool,
    injecting: AtomicBool,
    /// Bumped on anything that invalidates a pending match.
    generation: AtomicU64,
    stats: Mutex<NativeStats>,
    /// Raw CFMachPortRef of the active keyboard tap (0 when none).
    tap: AtomicUsize,
    /// Raw CFMachPortRef of the listen-only mouse tap (0 when none).
    mouse_tap: AtomicUsize,
    /// Raw CFRunLoopRef of the tap thread (0 when none).
    run_loop: AtomicUsize,
    /// Keyboard layout snapshot for empty-Unicode key events (refreshed on the main thread).
    layout: Mutex<Option<Arc<LayoutSnapshot>>>,
    /// When the pending match was queued (for cancellation timing diagnostics only).
    queued_at: Mutex<Option<Instant>>,
    /// Excluded bundle identifiers, lower-case.
    excluded: Mutex<HashSet<String>>,
    bundles: Mutex<HashMap<i32, (String, Vec<String>)>>,
    /// Frontmost app as reported by the UI (NSWorkspace), used when a key event carries no
    /// target PID. 0 = unknown.
    frontmost_pid: AtomicI32,
    /// App that received the previous key event (0 = none); a change resets matching.
    last_key_pid: AtomicI32,
    /// Target captured for Quick Search before its panel took keyboard focus.
    manual_target: Mutex<Option<ManualTarget>>,
    /// The Quick Search panel is open: automatic matching is suspended.
    inhibited: AtomicBool,
    next_token: AtomicU64,
}

struct ManualTarget {
    token: u64,
    focus: FocusIdentity,
    captured_at: Instant,
}

impl Shared {
    fn is_excluded_pid(&self, pid: i32) -> bool {
        let empty = self.excluded.lock().map(|e| e.is_empty()).unwrap_or(true);
        if empty || pid <= 0 {
            return false;
        }
        let ids = bundle_ids_for_pid(&self.bundles, pid);
        self.excluded
            .lock()
            .map(|e| is_excluded(&e, &ids))
            .unwrap_or(false)
    }

    fn cancel_pending(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
    }

    fn reset_engine(&self) {
        if let Ok(mut e) = self.engine.lock() {
            e.reset();
        }
    }

    fn gates(
        &self,
        job_generation: u64,
        target_pid: i32,
        focus_matches: Option<bool>,
    ) -> GateSnapshot {
        GateSnapshot {
            shutdown: self.shutdown.load(Ordering::SeqCst),
            paused: self.paused.load(Ordering::SeqCst),
            job_generation,
            current_generation: self.generation.load(Ordering::SeqCst),
            secure_input: secure_input_active(),
            excluded: self.is_excluded_pid(target_pid),
            focus_matches,
        }
    }
}

struct Job {
    expansion: Expansion,
    generation: u64,
    /// Target PID of the triggering key event, if macOS supplied one.
    target_pid: Option<i32>,
    /// Quick Search: the target captured before the panel opened. Injection waits for it to
    /// be focused again and never deletes anything.
    manual_origin: Option<FocusIdentity>,
}

struct TapContext {
    shared: Arc<Shared>,
    /// Bounded (capacity 1): at most one match is ever queued; the tap callback uses
    /// `try_send` so it never blocks the event stream.
    tx: SyncSender<Job>,
}

struct Threads {
    tap: JoinHandle<()>,
    injector: JoinHandle<()>,
}

pub struct NativeController {
    shared: Arc<Shared>,
    running: Option<Threads>,
    /// Sender used for Quick Search insertions (the tap thread owns its own clone).
    injector_tx: Option<SyncSender<Job>>,
    /// Threads that did not finish after a failed start; joined before the next start.
    orphans: Option<Threads>,
    /// A stop was begun (`begin_stop`) but not finished yet.
    stopping: bool,
    last_error: Option<String>,
}

impl Default for NativeController {
    fn default() -> Self {
        Self::new()
    }
}

impl NativeController {
    pub fn new() -> Self {
        Self {
            shared: Arc::new(Shared {
                engine: Mutex::new(Engine::default()),
                paused: AtomicBool::new(false),
                shutdown: AtomicBool::new(true),
                injecting: AtomicBool::new(false),
                generation: AtomicU64::new(0),
                stats: Mutex::new(NativeStats::default()),
                tap: AtomicUsize::new(0),
                mouse_tap: AtomicUsize::new(0),
                layout: Mutex::new(None),
                queued_at: Mutex::new(None),
                run_loop: AtomicUsize::new(0),
                excluded: Mutex::new(HashSet::new()),
                bundles: Mutex::new(HashMap::new()),
                frontmost_pid: AtomicI32::new(0),
                last_key_pid: AtomicI32::new(0),
                manual_target: Mutex::new(None),
                inhibited: AtomicBool::new(false),
                next_token: AtomicU64::new(1),
            }),
            running: None,
            injector_tx: None,
            orphans: None,
            stopping: false,
            last_error: None,
        }
    }

    pub fn permissions(&self) -> Permissions {
        permissions()
    }

    pub fn state(&self) -> NativeState {
        if self.stopping {
            return NativeState::Stopping;
        }
        if self.running.is_none() {
            return match &self.last_error {
                Some(err) => NativeState::Error(err.clone()),
                None => NativeState::Off,
            };
        }
        let tap = self.shared.tap.load(Ordering::SeqCst) as CFMachPortRef;
        // SAFETY: tap is valid while `running` is Some (released only after the thread exits).
        if !tap.is_null() && !unsafe { CGEventTapIsEnabled(tap) } {
            return NativeState::Disabled;
        }
        if self.shared.paused.load(Ordering::SeqCst) {
            NativeState::Paused
        } else {
            NativeState::Running
        }
    }

    /// Refresh the keyboard-layout snapshot. Call from the main (UI) thread only.
    pub fn refresh_keyboard_layout(&self) {
        if !is_main_thread() {
            return; // TIS is main-thread only; keep the previous snapshot.
        }
        let snap = current_layout_snapshot().map(Arc::new);
        if let Ok(mut l) = self.shared.layout.lock() {
            *l = snap;
        }
    }

    pub fn stats(&self) -> NativeStats {
        self.shared
            .stats
            .lock()
            .map(|s| s.clone())
            .unwrap_or_default()
    }

    pub fn update_library(&self, library: &Library) {
        self.shared.cancel_pending();
        if let Ok(mut e) = self.shared.engine.lock() {
            e.load(library);
            e.reset();
        }
    }

    pub fn set_paused(&mut self, paused: bool) {
        self.shared.paused.store(paused, Ordering::SeqCst);
        self.shared.cancel_pending();
        self.shared.reset_engine();
    }

    /// Replace the excluded bundle identifiers (lower-cased). Cancels anything pending.
    pub fn set_exclusions(&self, excluded: HashSet<String>) {
        if let Ok(mut e) = self.shared.excluded.lock() {
            *e = excluded;
        }
        self.shared.cancel_pending();
        self.shared.reset_engine();
    }

    /// The frontmost app changed (reported by the UI). Typed context is gone, so matching
    /// resets. A pending injection is not cancelled here: the injector re-checks the focused
    /// app and field before every key, and a Quick Search insertion legitimately waits for
    /// its original app to become frontmost again.
    pub fn set_frontmost(&self, pid: i32) {
        let previous = self.shared.frontmost_pid.swap(pid.max(0), Ordering::SeqCst);
        if previous != pid {
            self.shared.reset_engine();
        }
    }

    /// Capture the focused app and text element for Quick Search, before its panel takes
    /// keyboard focus. Needs Accessibility (read-only AX queries; nothing is typed). The
    /// result says whether insertion could be allowed; Copy works regardless.
    pub fn capture_target(&self) -> Result<CapturedTarget, String> {
        if !permissions().accessibility {
            return Err("Accessibility permission is not granted, so the field you came from cannot be identified. Copy still works.".into());
        }
        let focus = FocusIdentity::capture()
            .ok_or("The focused app could not be identified (fail closed). Copy still works.")?;
        let own_app = focus.pid as i64 == std::process::id() as i64;
        let ids = bundle_ids_for_pid(&self.shared.bundles, focus.pid);
        let target = CapturedTarget {
            token: self.shared.next_token.fetch_add(1, Ordering::SeqCst),
            pid: focus.pid,
            bundle_id: ids.first().cloned(),
            excluded: self.shared.is_excluded_pid(focus.pid),
            secure_input: secure_input_active(),
            own_app,
            has_focused_element: !focus.element.is_null(),
        };
        if let Ok(mut m) = self.shared.manual_target.lock() {
            *m = Some(ManualTarget {
                token: target.token,
                focus,
                captured_at: Instant::now(),
            });
        }
        Ok(target)
    }

    /// Suspend (or resume) automatic matching while the Quick Search panel is open. The
    /// panel does not activate Typelune, so its search typing would otherwise look like
    /// typing in the app underneath. Running/Paused state is not changed.
    pub fn set_inhibited(&self, on: bool) {
        self.shared.inhibited.store(on, Ordering::SeqCst);
        self.shared.reset_engine();
    }

    pub fn is_inhibited(&self) -> bool {
        self.shared.inhibited.load(Ordering::SeqCst)
    }

    /// Forget a captured target (panel closed without inserting).
    pub fn release_target(&self) {
        if let Ok(mut m) = self.shared.manual_target.lock() {
            *m = None;
        }
    }

    /// Queue a Quick Search insertion of `snippet` into the target captured with `token`.
    /// Refused unless expansion is running (not paused/stopping) and the target is fresh,
    /// not Typelune, not excluded and Secure Keyboard Entry is off. Nothing is erased. The
    /// injector re-verifies everything before and during typing.
    pub fn quick_insert(&mut self, token: u64, snippet: &Snippet) -> Result<(), String> {
        let held = self
            .shared
            .manual_target
            .lock()
            .ok()
            .and_then(|mut m| m.take());
        let state = ManualInsertState {
            running: self.running.is_some() && !self.shared.shutdown.load(Ordering::SeqCst),
            stopping: self.stopping,
            paused: self.shared.paused.load(Ordering::SeqCst),
            held_token: held.as_ref().map(|t| t.token),
            requested_token: token,
            target_age: held
                .as_ref()
                .map(|t| t.captured_at.elapsed())
                .unwrap_or_default(),
            excluded: held
                .as_ref()
                .is_some_and(|t| self.shared.is_excluded_pid(t.focus.pid)),
            secure_input: secure_input_active(),
            own_app: held
                .as_ref()
                .is_some_and(|t| t.focus.pid as i64 == std::process::id() as i64),
            exact_field: held.as_ref().is_some_and(|t| !t.focus.element.is_null()),
        };
        check_manual_insert(&state).map_err(|r| r.describe().to_owned())?;
        let (Some(target), Some(tx)) = (held, self.injector_tx.as_ref()) else {
            return Err(super::ManualRefusal::NotRunning.describe().into());
        };
        if self.shared.injecting.swap(true, Ordering::SeqCst) {
            return Err("Another expansion is being typed; try again in a moment.".into());
        }
        let job = Job {
            expansion: Expansion {
                snippet_id: snippet.id,
                title: snippet.display_name(),
                abbreviation: String::new(),
                typed: String::new(),
                body: snippet.body.clone(),
                suffix: String::new(),
            },
            generation: self.shared.generation.load(Ordering::SeqCst),
            target_pid: None,
            manual_origin: Some(target.focus),
        };
        if let Ok(mut q) = self.shared.queued_at.lock() {
            *q = Some(Instant::now());
        }
        match tx.try_send(job) {
            Ok(()) => {
                bump(&self.shared, |d| d.quick_inserts_queued += 1);
                Ok(())
            }
            Err(_) => {
                self.shared.injecting.store(false, Ordering::SeqCst);
                Err("The injector is busy or stopped; nothing was typed.".into())
            }
        }
    }

    pub fn start(&mut self, library: &Library) -> Result<(), String> {
        if self.running.is_some() {
            return Ok(());
        }
        if self.stopping {
            return Err("System-wide expansion is still stopping; try again in a moment.".into());
        }
        if let Some(o) = self.orphans.take() {
            if o.tap.is_finished() && o.injector.is_finished() {
                let _ = o.tap.join();
                let _ = o.injector.join();
            } else {
                self.orphans = Some(o);
                let msg =
                    "A previous event-tap thread is still shutting down; try again in a moment."
                        .to_owned();
                self.last_error = Some(msg.clone());
                return Err(msg);
            }
        }
        if !is_main_thread() {
            let msg = "System-wide expansion must be started from the main thread.".to_owned();
            self.last_error = Some(msg.clone());
            return Err(msg);
        }
        self.last_error = None;
        if !permissions().accessibility {
            let msg = "Accessibility permission is not granted to this app. Grant it manually in System Settings → Privacy & Security → Accessibility, then press Start again.".to_owned();
            self.last_error = Some(msg.clone());
            return Err(msg);
        }
        self.update_library(library);
        if let Ok(mut s) = self.shared.stats.lock() {
            *s = NativeStats::default();
        }
        // start() runs on the UI thread, so the TIS snapshot is taken on the main thread.
        self.refresh_keyboard_layout();
        self.shared.paused.store(false, Ordering::SeqCst);
        self.shared.injecting.store(false, Ordering::SeqCst);
        self.shared.shutdown.store(false, Ordering::SeqCst);
        self.shared.last_key_pid.store(0, Ordering::SeqCst);

        let (tx, rx) = mpsc::sync_channel::<Job>(INJECT_QUEUE_CAPACITY);
        let manual_tx = tx.clone();
        let (ready_tx, ready_rx) = mpsc::channel::<Result<(), String>>();
        let shared = self.shared.clone();
        let tap = std::thread::Builder::new()
            .name("typelune-event-tap".into())
            .spawn(move || run_tap(shared, tx, ready_tx))
            .map_err(|e| e.to_string())?;
        let shared = self.shared.clone();
        let injector = match std::thread::Builder::new()
            .name("typelune-injector".into())
            .spawn(move || run_injector(shared, rx))
        {
            Ok(h) => h,
            Err(e) => {
                self.shared.shutdown.store(true, Ordering::SeqCst);
                let _ = tap.join();
                return Err(e.to_string());
            }
        };
        let threads = Threads { tap, injector };

        let outcome = match ready_rx.recv_timeout(START_TIMEOUT) {
            Ok(Ok(())) => {
                self.running = Some(threads);
                self.injector_tx = Some(manual_tx);
                return Ok(());
            }
            Ok(Err(msg)) => msg,
            Err(_) => "Event tap did not start within 3 s; startup was cancelled.".to_owned(),
        };
        // Failed start: make sure both threads end; never leave a live tap behind.
        self.shutdown_threads(threads);
        self.last_error = Some(outcome.clone());
        Err(outcome)
    }

    /// Synchronous stop (signal + join, bounded by 2 s). Used at quit/drop and by the legacy UI.
    pub fn stop(&mut self) {
        if let Some(pending) = self.begin_stop() {
            let outcome = pending.wait();
            self.finish_stop(outcome);
        }
        self.shared.reset_engine();
    }

    /// First half of a stop that must not block the caller's lock: immediately signals
    /// shutdown (disables the tap, cancels any pending/in-progress injection, stops the run
    /// loop) and hands back the threads to join. Start is refused until `finish_stop`.
    pub fn begin_stop(&mut self) -> Option<PendingStop> {
        let threads = self.running.take()?;
        self.injector_tx = None;
        self.release_target();
        self.signal_shutdown();
        self.stopping = true;
        self.shared.reset_engine();
        Some(PendingStop { threads })
    }

    /// Second half: record threads that did not finish (joined before the next start).
    pub fn finish_stop(&mut self, outcome: StopOutcome) {
        if let Some(t) = outcome.leftover {
            self.orphans = Some(t);
        }
        self.stopping = false;
        self.shared.injecting.store(false, Ordering::SeqCst);
        self.shared.reset_engine();
    }

    pub fn is_stopping(&self) -> bool {
        self.stopping
    }

    fn shutdown_threads(&mut self, t: Threads) {
        self.signal_shutdown();
        if let Some(t) = wait_threads(t) {
            self.orphans = Some(t);
        }
        self.shared.injecting.store(false, Ordering::SeqCst);
    }

    fn signal_shutdown(&self) {
        self.shared.shutdown.store(true, Ordering::SeqCst);
        self.shared.cancel_pending();
        let tap = self.shared.tap.load(Ordering::SeqCst) as CFMachPortRef;
        let rl = self.shared.run_loop.load(Ordering::SeqCst) as CFRunLoopRef;
        // SAFETY: tap/run loop pointers are published by the tap thread and cleared by it
        // before release; the run loop polls `shutdown` so a missed stop is harmless.
        unsafe {
            if !tap.is_null() {
                CGEventTapEnable(tap, false);
            }
            if !rl.is_null() {
                CFRunLoopStop(rl);
            }
        }
    }
}

/// Threads of a stop in progress; join them without holding the controller.
pub struct PendingStop {
    threads: Threads,
}

/// Result of [`PendingStop::wait`]: threads that did not finish in time, if any.
pub struct StopOutcome {
    leftover: Option<Threads>,
}

impl PendingStop {
    /// Join the tap and injector threads, bounded by 2 s.
    pub fn wait(self) -> StopOutcome {
        StopOutcome {
            leftover: wait_threads(self.threads),
        }
    }
}

/// Wait up to 2 s for both threads, then join them. Returns them if they did not finish.
fn wait_threads(t: Threads) -> Option<Threads> {
    let deadline = Instant::now() + Duration::from_secs(2);
    while !(t.tap.is_finished() && t.injector.is_finished()) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    if t.tap.is_finished() && t.injector.is_finished() {
        let _ = t.tap.join();
        let _ = t.injector.join();
        None
    } else {
        Some(t)
    }
}

impl Drop for NativeController {
    fn drop(&mut self) {
        self.stop();
    }
}

// ---- Tap thread -----------------------------------------------------------------------

fn run_tap(shared: Arc<Shared>, tx: SyncSender<Job>, ready: Sender<Result<(), String>>) {
    let ctx = Box::into_raw(Box::new(TapContext {
        shared: shared.clone(),
        tx,
    }));
    // The keyboard tap has a keyboard-only mask on purpose. If macOS does not permit key
    // monitoring it clears those mask bits; with mouse bits in the same mask the tap would
    // still be created (and look "Running") while never seeing a key. Keyboard-only means
    // that case returns NULL and is reported honestly.
    let key_mask: u64 = 1 << EV_KEY_DOWN;
    let mouse_mask: u64 =
        (1 << EV_LEFT_MOUSE_DOWN) | (1 << EV_RIGHT_MOUSE_DOWN) | (1 << EV_OTHER_MOUSE_DOWN);
    // SAFETY: FFI calls with valid arguments; `ctx` outlives the run loop and is freed after
    // both taps are disabled and invalidated, so the callback can no longer observe it.
    unsafe {
        let tap = CGEventTapCreate(
            SESSION_EVENT_TAP,
            HEAD_INSERT,
            TAP_OPTION_DEFAULT,
            key_mask,
            tap_callback,
            ctx as *mut c_void,
        );
        if tap.is_null() {
            drop(Box::from_raw(ctx));
            let _ = ready.send(Err(
                "macOS refused the keyboard event tap (CGEventTapCreate returned NULL for key events). Accessibility for this exact app build is missing or stale; re-check it in System Settings.".into(),
            ));
            return;
        }
        // Mouse clicks only reset matching; a listen-only tap never delays clicks.
        let mouse_tap = CGEventTapCreate(
            SESSION_EVENT_TAP,
            HEAD_INSERT,
            TAP_OPTION_LISTEN_ONLY,
            mouse_mask,
            tap_callback,
            ctx as *mut c_void,
        );
        if mouse_tap.is_null() {
            CFMachPortInvalidate(tap);
            CFRelease(tap as CFTypeRef);
            drop(Box::from_raw(ctx));
            let _ = ready.send(Err(
                "Could not create the mouse-click tap used to reset matching; not starting (fail closed).".into(),
            ));
            return;
        }
        let source = CFMachPortCreateRunLoopSource(std::ptr::null(), tap, 0);
        let mouse_source = CFMachPortCreateRunLoopSource(std::ptr::null(), mouse_tap, 0);
        if source.is_null() || mouse_source.is_null() {
            for (port, src) in [(tap, source), (mouse_tap, mouse_source)] {
                if !src.is_null() {
                    CFRelease(src as CFTypeRef);
                }
                CFMachPortInvalidate(port);
                CFRelease(port as CFTypeRef);
            }
            drop(Box::from_raw(ctx));
            let _ = ready.send(Err(
                "Could not create run loop sources for event taps".into()
            ));
            return;
        }
        let rl = CFRunLoopGetCurrent();
        CFRunLoopAddSource(rl, source, kCFRunLoopCommonModes);
        CFRunLoopAddSource(rl, mouse_source, kCFRunLoopCommonModes);
        shared.tap.store(tap as usize, Ordering::SeqCst);
        shared.mouse_tap.store(mouse_tap as usize, Ordering::SeqCst);
        shared.run_loop.store(rl as usize, Ordering::SeqCst);

        // Startup may have been cancelled (timeout) before we got here.
        if !shared.shutdown.load(Ordering::SeqCst) {
            CGEventTapEnable(tap, true);
            CGEventTapEnable(mouse_tap, true);
            let _ = ready.send(Ok(()));
            while !shared.shutdown.load(Ordering::SeqCst) {
                CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.2, 0);
            }
        }

        CGEventTapEnable(tap, false);
        CGEventTapEnable(mouse_tap, false);
        shared.tap.store(0, Ordering::SeqCst);
        shared.mouse_tap.store(0, Ordering::SeqCst);
        shared.run_loop.store(0, Ordering::SeqCst);
        for (port, src) in [(tap, source), (mouse_tap, mouse_source)] {
            CFMachPortInvalidate(port);
            CFRelease(src as CFTypeRef);
            CFRelease(port as CFTypeRef);
        }
        // Dropping the context drops the Sender, which also ends the injector thread.
        drop(Box::from_raw(ctx));
    }
}

extern "C" fn tap_callback(
    _proxy: CGEventTapProxy,
    event_type: u32,
    event: CGEventRef,
    user_info: *mut c_void,
) -> CGEventRef {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // SAFETY: user_info is the TapContext pointer we registered; valid while the tap runs.
        let ctx = unsafe { &*(user_info as *const TapContext) };
        handle_event(ctx, event_type, event);
    }));
    // Always pass the event through unchanged.
    event
}

fn bump(shared: &Shared, f: impl FnOnce(&mut NativeDiagnostics)) {
    if let Ok(mut s) = shared.stats.lock() {
        f(&mut s.diag);
    }
}

/// Non-content fields read from a key event.
struct KeyEventInfo {
    marker: i64,
    source_state_id: i64,
    source_pid: i64,
    target_pid: i64,
    keycode: u16,
    flags: u64,
    chars: String,
}

/// Read the fields we need from a keyboard event. The Unicode string is read at its full
/// reported length (capped), so multi-character synthetic events are not truncated.
///
/// # Safety
/// `event` must be a valid CGEventRef.
unsafe fn read_key_event(event: CGEventRef) -> KeyEventInfo {
    let mut len: usize = 0;
    CGEventKeyboardGetUnicodeString(event, 0, &mut len, std::ptr::null_mut());
    let mut buf = vec![0u16; len.clamp(1, MAX_EVENT_UTF16)];
    let mut got: usize = 0;
    CGEventKeyboardGetUnicodeString(event, buf.len(), &mut got, buf.as_mut_ptr());
    KeyEventInfo {
        marker: CGEventGetIntegerValueField(event, FIELD_SOURCE_USER_DATA),
        source_state_id: CGEventGetIntegerValueField(event, FIELD_SOURCE_STATE_ID),
        source_pid: CGEventGetIntegerValueField(event, FIELD_SOURCE_UNIX_PID),
        target_pid: CGEventGetIntegerValueField(event, FIELD_TARGET_UNIX_PID),
        keycode: CGEventGetIntegerValueField(event, FIELD_KEYCODE) as u16,
        flags: CGEventGetFlags(event),
        chars: String::from_utf16_lossy(&buf[..got.min(buf.len())]),
    }
}

fn is_own_event(info: &KeyEventInfo) -> bool {
    info.marker == SYNTHETIC_MARKER || info.source_pid == std::process::id() as i64
}

/// Resolve the receiving process without substituting the active application.
fn receiving_pid(target_pid: i64, focused_pid: impl FnOnce() -> Option<i32>) -> Option<i32> {
    if target_pid > 0 {
        // Invalid explicit targets fail closed; never wrap a PID or replace it with focus.
        i32::try_from(target_pid).ok()
    } else {
        Some(focused_pid().filter(|pid| *pid > 0).unwrap_or(0))
    }
}

/// Key events delivered to this app are not matched globally (the app has its own
/// Playground engine). Only a positive, exact PID match counts; 0 means "unknown".
fn targets_own_process(target_pid: i64, own_pid: u32) -> bool {
    target_pid > 0 && target_pid == own_pid as i64
}

fn elapsed_since_queue(shared: &Shared) -> u128 {
    shared
        .queued_at
        .lock()
        .ok()
        .and_then(|q| *q)
        .map(|t| t.elapsed().as_millis())
        .unwrap_or(0)
}

/// Bounded, non-content description of a key event that cancelled an injection:
/// source PID (and whether it is us), marker match, source state ID, category, timing.
/// Never includes characters or key codes.
fn describe_cancel_event(info: &KeyEventInfo, category: KeyCategory, elapsed_ms: u128) -> String {
    let category = match category {
        KeyCategory::Text => "text",
        KeyCategory::EmptyText => "empty-unicode",
        KeyCategory::Reset => "reset",
        KeyCategory::Backspace => "backspace",
    };
    format!(
        "key ({category}), source pid {} (self: {}), marker match: {}, source state {}, +{elapsed_ms} ms after match",
        info.source_pid,
        info.source_pid == std::process::id() as i64,
        info.marker == SYNTHETIC_MARKER,
        info.source_state_id,
    )
}

fn handle_event(ctx: &TapContext, event_type: u32, event: CGEventRef) {
    let shared = &ctx.shared;
    match event_type {
        EV_TAP_DISABLED_BY_TIMEOUT | EV_TAP_DISABLED_BY_USER_INPUT => {
            shared.cancel_pending();
            shared.reset_engine();
            if !shared.shutdown.load(Ordering::SeqCst) {
                for port in [&shared.tap, &shared.mouse_tap] {
                    let tap = port.load(Ordering::SeqCst) as CFMachPortRef;
                    if !tap.is_null() {
                        // SAFETY: taps are valid while running.
                        unsafe { CGEventTapEnable(tap, true) };
                    }
                }
                if let Ok(mut s) = shared.stats.lock() {
                    s.tap_reenabled += 1;
                }
            }
            return;
        }
        EV_LEFT_MOUSE_DOWN | EV_RIGHT_MOUSE_DOWN | EV_OTHER_MOUSE_DOWN => {
            let during = shared.injecting.load(Ordering::SeqCst);
            shared.cancel_pending();
            shared.reset_engine();
            let elapsed = elapsed_since_queue(shared);
            bump(shared, |d| {
                d.mouse_resets += 1;
                if during {
                    d.mouse_during_injection += 1;
                    d.last_cancel_event = Some(format!("mouse click, +{elapsed} ms after match"));
                }
            });
            return;
        }
        EV_KEY_DOWN => {}
        _ => return,
    }
    if event.is_null() || shared.shutdown.load(Ordering::SeqCst) {
        return;
    }
    bump(shared, |d| d.key_events_seen += 1);
    // SAFETY: event is a valid CGEventRef for the duration of the callback.
    let mut info = unsafe { read_key_event(event) };
    if is_own_event(&info) {
        bump(shared, |d| d.own_event_skips += 1);
        return;
    }
    if shared.injecting.load(Ordering::SeqCst) {
        // Real (non-own) key while an expansion is pending/in progress: cancel it.
        shared.cancel_pending();
        shared.reset_engine();
        let note = describe_cancel_event(
            &info,
            key_category(info.keycode, info.flags, &info.chars),
            elapsed_since_queue(shared),
        );
        bump(shared, |d| {
            d.typed_during_injection += 1;
            d.last_cancel_event = Some(note);
        });
        return;
    }
    // An explicit event target remains authoritative. Without one, use keyboard
    // focus rather than NSWorkspace's active app (which can sit behind a panel).
    let Some(key_pid) = receiving_pid(info.target_pid, keyboard_focus_pid) else {
        shared.cancel_pending();
        shared.reset_engine();
        bump(shared, |d| d.unknown_target_skips += 1);
        return;
    };
    if key_pid > 0 {
        let previous = shared.last_key_pid.swap(key_pid, Ordering::SeqCst);
        if previous != 0 && previous != key_pid {
            // Typing moved to another app: what was typed before is not context any more.
            shared.cancel_pending();
            shared.reset_engine();
            bump(shared, |d| d.focus_change_resets += 1);
        }
    }
    let exclusions_set = shared
        .excluded
        .lock()
        .map(|e| !e.is_empty())
        .unwrap_or(true);
    let paused = shared.paused.load(Ordering::SeqCst);
    let inhibited = shared.inhibited.load(Ordering::SeqCst);
    let own_app = targets_own_process(key_pid as i64, std::process::id());
    let context = KeyContext {
        paused,
        inhibited,
        // Only queried when it can still matter (it is a cheap system call).
        secure_input: !paused && !inhibited && secure_input_active(),
        own_app,
        key_pid,
        exclusions_set,
        excluded: !paused
            && !inhibited
            && !own_app
            && key_pid > 0
            && shared.is_excluded_pid(key_pid),
    };
    if let Some(skip) = auto_skip(&context) {
        if skip != AutoSkip::Paused {
            shared.cancel_pending();
        }
        shared.reset_engine();
        match skip {
            AutoSkip::Paused => bump(shared, |d| d.paused_skips += 1),
            AutoSkip::Inhibited => bump(shared, |d| d.inhibited_skips += 1),
            AutoSkip::SecureInput => {
                if let Ok(mut s) = shared.stats.lock() {
                    s.secure_input_skips += 1;
                }
            }
            // Typing into our own editor/Playground: never feed the global matcher, otherwise
            // the in-app engine and the global injector would both expand the same text.
            AutoSkip::OwnApp => bump(shared, |d| d.own_app_skips += 1),
            AutoSkip::UnknownTarget => bump(shared, |d| d.unknown_target_skips += 1),
            AutoSkip::ExcludedApp => bump(shared, |d| d.excluded_skips += 1),
        }
        return;
    }
    let mut category = key_category(info.keycode, info.flags, &info.chars);
    if category == KeyCategory::EmptyText {
        // Keycode-only event: translate with the cached layout instead of resetting.
        let layout = shared.layout.lock().ok().and_then(|l| l.clone());
        if let Some(layout) = layout {
            let text = translate_keycode(&layout, info.keycode, info.flags);
            if !text.is_empty() {
                info.chars = text;
                category = key_category(info.keycode, info.flags, &info.chars);
                bump(shared, |d| d.layout_translated += 1);
            }
        }
    }
    bump(shared, |d| {
        if info.target_pid > 0 {
            d.target_pid_present += 1;
        }
        match category {
            KeyCategory::Text => d.text_inputs += 1,
            KeyCategory::EmptyText => d.empty_text_events += 1,
            KeyCategory::Reset => d.reset_inputs += 1,
            KeyCategory::Backspace => d.backspaces += 1,
        }
    });
    let Ok(mut engine) = shared.engine.lock() else {
        return;
    };
    for input in classify_key(info.keycode, info.flags, &info.chars) {
        if input == KeyInput::Reset {
            shared.cancel_pending();
        }
        if let Some(expansion) = engine.feed(input) {
            let job = Job {
                expansion,
                generation: shared.generation.load(Ordering::SeqCst),
                // Bound to the app the key was attributed to; the injector refuses any other.
                target_pid: (key_pid > 0).then_some(key_pid),
                manual_origin: None,
            };
            shared.injecting.store(true, Ordering::SeqCst);
            if let Ok(mut q) = shared.queued_at.lock() {
                *q = Some(Instant::now());
            }
            match ctx.tx.try_send(job) {
                Ok(()) => bump(shared, |d| d.matches_queued += 1),
                // Full cannot normally happen (the `injecting` gate allows one job in flight);
                // either way the match is dropped rather than blocking the tap callback.
                Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => {
                    shared.injecting.store(false, Ordering::SeqCst);
                }
            }
            break;
        }
    }
}

// ---- Injector thread ------------------------------------------------------------------

fn run_injector(shared: Arc<Shared>, rx: Receiver<Job>) {
    let source = make_synthetic_source();
    loop {
        let job = match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(job) => job,
            Err(RecvTimeoutError::Timeout) => {
                if shared.shutdown.load(Ordering::SeqCst) {
                    break;
                }
                continue;
            }
            Err(RecvTimeoutError::Disconnected) => break,
        };
        if shared.shutdown.load(Ordering::SeqCst) {
            break;
        }
        let result = inject(&shared, source, &job);
        let manual = job.manual_origin.is_some();
        if let Ok(mut s) = shared.stats.lock() {
            let now = chrono::Local::now();
            match result {
                Ok(warning) => {
                    s.expansions += 1;
                    s.last_snippet = Some(job.expansion.title.clone());
                    s.last_at = Some(now);
                    s.last_warning = warning;
                    if manual {
                        s.last_quick_insert = Some(format!(
                            "Inserted “{}” at {}",
                            job.expansion.title,
                            now.format("%H:%M:%S")
                        ));
                    }
                }
                Err((reason, partial, detail)) => {
                    s.aborted += 1;
                    let text = format!(
                        "{}{}{} at {}",
                        reason.describe(),
                        detail.map(|d| format!(" [{d}]")).unwrap_or_default(),
                        if partial {
                            " (after partial injection)"
                        } else {
                            ""
                        },
                        now.format("%H:%M:%S")
                    );
                    if manual {
                        s.last_quick_insert = Some(format!("Not inserted: {text}"));
                    }
                    s.last_abort = Some(text);
                }
            }
        }
        shared.reset_engine();
        shared.injecting.store(false, Ordering::SeqCst);
    }
    // Discard anything still queued; nothing is injected after stop.
    while rx.try_recv().is_ok() {}
    shared.injecting.store(false, Ordering::SeqCst);
    if !source.is_null() {
        // SAFETY: we own this reference.
        unsafe { CFRelease(source as CFTypeRef) };
    }
}

/// Inject one expansion. Returns the first render warning, or why it was aborted
/// (and whether some synthetic keys had already been sent).
fn inject(
    shared: &Shared,
    source: CGEventSourceRef,
    job: &Job,
) -> Result<Option<String>, (AbortReason, bool, Option<String>)> {
    let captured;
    let origin: &FocusIdentity = match &job.manual_origin {
        Some(target) => {
            // Quick Search: the panel has closed; wait (bounded) for the exact app and field
            // captured before it opened to be focused again. Never type anywhere else.
            let early = |pid| evaluate_gates(&shared.gates(job.generation, pid, Some(true)));
            let deadline = Instant::now() + MANUAL_REFOCUS_TIMEOUT;
            loop {
                early(target.pid).map_err(|r| (r, false, None))?;
                if FocusIdentity::capture().is_some_and(|now| target.matches(&now)) {
                    break;
                }
                if Instant::now() >= deadline {
                    return Err((
                        AbortReason::FocusChanged,
                        false,
                        Some("the original field was not focused again".into()),
                    ));
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            target
        }
        None => {
            captured = FocusIdentity::capture().ok_or((AbortReason::FocusUnknown, false, None))?;
            &captured
        }
    };
    let quick = || evaluate_gates(&shared.gates(job.generation, origin.pid, Some(true)));
    quick().map_err(|r| (r, false, None))?;
    if origin.pid as i64 == std::process::id() as i64 {
        // Never inject into ourselves (covers events that carried no target PID).
        return Err((AbortReason::OwnApp, false, None));
    }
    if let Some(target) = job.target_pid {
        if target != origin.pid {
            // PIDs only (no content): helps tell automation routing from real focus changes.
            let detail = format!("event target pid {target}, focused app pid {}", origin.pid);
            return Err((AbortReason::TargetMismatch, false, Some(detail)));
        }
    }
    // Every gate, including the focused app *and* field, is re-verified immediately before
    // each synthetic key event: each Backspace, Return and Left arrow, and each text chunk
    // (at most MAX_UTF16_PER_EVENT UTF-16 units). One event cannot be checked part-way.
    let full = || {
        let now = FocusIdentity::capture();
        let focus = now.map(|n| origin.matches(&n));
        evaluate_gates(&shared.gates(job.generation, origin.pid, focus))
    };

    // Let the trigger key land (or the refocused field settle), watching for cancellation.
    let settle = if job.manual_origin.is_some() {
        MANUAL_SETTLE_DELAY
    } else {
        SETTLE_DELAY
    };
    let settle_until = Instant::now() + settle;
    while Instant::now() < settle_until {
        quick().map_err(|r| (r, false, None))?;
        std::thread::sleep(Duration::from_millis(5));
    }
    full().map_err(|r| (r, false, None))?;

    let clipboard = if uses_clipboard(&job.expansion.body) {
        arboard::Clipboard::new()
            .ok()
            .and_then(|mut c| c.get_text().ok())
    } else {
        None
    };
    let rep = job
        .expansion
        .render(&RenderContext::new(chrono::Local::now(), clipboard));

    // Deliver straight to the AX-verified focused process. Per the SDK, CGEventPostToPid
    // posts "immediately before any event taps instantiated for the specified process", so
    // per that contract these events should not re-enter our session tap. v0.1.1 posted at
    // the HID tap and its injection in Notes was cancelled after partial injection; the
    // cancelling event was not captured. HYPOTHESIS (unproven): it was our own Backspace
    // seen as foreign. PostToPid delivery itself still needs a real Notes test.
    let pid = origin.pid;
    let mut sink = |ev: CGEventRef| {
        // SAFETY: ev is a valid event owned by the caller.
        unsafe { CGEventPostToPid(pid, ev) };
        std::thread::sleep(Duration::from_millis(1));
    };
    run_steps(source, &plan_injection(&rep), &full, &mut sink)
        .map_err(|(r, partial)| (r, partial, None))?;
    std::thread::sleep(Duration::from_millis(20));
    Ok(rep.warnings.first().cloned())
}

/// Private event source whose events all carry [`SYNTHETIC_MARKER`] as user data
/// (`CGEventSourceSetUserData`: "sets the user-specified data for all events created by
/// the specified event source").
fn make_synthetic_source() -> CGEventSourceRef {
    // SAFETY: creating a source has no preconditions; the caller releases it.
    unsafe {
        let source = CGEventSourceCreate(SOURCE_STATE_PRIVATE);
        if !source.is_null() {
            CGEventSourceSetUserData(source, SYNTHETIC_MARKER);
        }
        source
    }
}

/// Create one tagged synthetic keyboard event (flags cleared, marker set on the event too).
/// Returns null on failure; the caller releases the event.
fn make_synthetic_event(
    source: CGEventSourceRef,
    keycode: u16,
    down: bool,
    text: Option<&[u16]>,
) -> CGEventRef {
    // SAFETY: plain event construction; buffer outlives the call.
    unsafe {
        let ev = CGEventCreateKeyboardEvent(source, keycode, down);
        if ev.is_null() {
            return ev;
        }
        CGEventSetFlags(ev, 0);
        if let Some(t) = text {
            CGEventKeyboardSetUnicodeString(ev, t.len(), t.as_ptr());
        }
        CGEventSetIntegerValueField(ev, FIELD_SOURCE_USER_DATA, SYNTHETIC_MARKER);
        ev
    }
}

/// Emit the down/up pair for one key (or text chunk) into `sink`, then release them.
fn emit_key(
    source: CGEventSourceRef,
    keycode: u16,
    text: Option<&[u16]>,
    sink: &mut dyn FnMut(CGEventRef),
) {
    for down in [true, false] {
        let ev = make_synthetic_event(source, keycode, down, text);
        if ev.is_null() {
            continue;
        }
        sink(ev);
        // SAFETY: we own the event; sinks never retain it.
        unsafe { CFRelease(ev as CFTypeRef) };
    }
}

/// Emit all steps, checking `gate` before every key event. On failure, reports whether any
/// event had already been delivered.
fn run_steps(
    source: CGEventSourceRef,
    steps: &[InjectStep],
    gate: &dyn Fn() -> Result<(), AbortReason>,
    sink: &mut dyn FnMut(CGEventRef),
) -> Result<(), (AbortReason, bool)> {
    let emitted = std::cell::Cell::new(0usize);
    let mut counting = |ev: CGEventRef| {
        emitted.set(emitted.get() + 1);
        sink(ev);
    };
    for step in steps {
        post_step(source, step, gate, &mut counting).map_err(|r| (r, emitted.get() > 0))?;
    }
    Ok(())
}

/// Emit one injection step. `gate` is checked before every key; `sink` delivers events
/// (posting in production, capturing in tests).
fn post_step(
    source: CGEventSourceRef,
    step: &InjectStep,
    gate: &dyn Fn() -> Result<(), AbortReason>,
    sink: &mut dyn FnMut(CGEventRef),
) -> Result<(), AbortReason> {
    match step {
        InjectStep::Backspaces(n) => {
            for _ in 0..*n {
                gate()?;
                emit_key(source, super::keycode::DELETE, None, sink);
            }
        }
        InjectStep::Return => {
            gate()?;
            emit_key(source, super::keycode::RETURN, None, sink);
        }
        InjectStep::Left(n) => {
            for _ in 0..*n {
                gate()?;
                emit_key(source, super::keycode::LEFT, None, sink);
            }
        }
        InjectStep::Text(text) => {
            gate()?;
            let utf16: Vec<u16> = text.encode_utf16().collect();
            emit_key(source, 0, Some(&utf16), sink);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    //! These tests only *create* CGEvent objects in-process to exercise our field/Unicode
    //! reading. Nothing is posted, no tap is created, and no permission is needed.
    use super::*;

    fn make_key_event(keycode: u16, text: Option<&str>) -> CGEventRef {
        // SAFETY: plain event construction; caller releases.
        unsafe {
            let ev = CGEventCreateKeyboardEvent(std::ptr::null_mut(), keycode, true);
            assert!(!ev.is_null());
            if let Some(t) = text {
                let u: Vec<u16> = t.encode_utf16().collect();
                CGEventKeyboardSetUnicodeString(ev, u.len(), u.as_ptr());
            }
            ev
        }
    }

    fn release(ev: CGEventRef) {
        // SAFETY: we own the event.
        unsafe { CFRelease(ev as CFTypeRef) };
    }

    #[test]
    fn only_absent_ax_attributes_allow_focus_fallback() {
        assert_eq!(ax_attribute_present(0), Ok(true));
        assert_eq!(ax_attribute_present(-25205), Ok(false)); // unsupported
        assert_eq!(ax_attribute_present(-25212), Ok(false)); // no value
        assert_eq!(ax_attribute_present(-25204), Err(())); // timeout/cannotComplete
        assert_eq!(ax_attribute_present(-25202), Err(())); // invalid element
        assert_eq!(ax_attribute_present(-25211), Err(())); // API disabled
    }

    #[test]
    fn panel_focus_uses_element_owner_and_rejects_inconsistent_fallback() {
        assert_eq!(focused_owner(Some(Some(42)), None), Some(42));
        assert_eq!(focused_owner(Some(Some(42)), Some(42)), Some(42));
        assert_eq!(focused_owner(Some(Some(42)), Some(7)), None);
        assert_eq!(focused_owner(Some(None), Some(7)), None);
        assert_eq!(focused_owner(Some(Some(0)), None), None);
        assert_eq!(focused_owner(None, Some(7)), Some(7));
        assert_eq!(focused_owner(None, None), None);
    }

    #[test]
    fn receiving_process_uses_focus_only_without_an_explicit_target() {
        assert_eq!(receiving_pid(0, || Some(42)), Some(42));
        assert_eq!(
            receiving_pid(7, || panic!("must not replace event target")),
            Some(7)
        );
        assert_eq!(receiving_pid(i64::MAX, || Some(42)), None);
        assert_eq!(receiving_pid(0, || None), Some(0));
        assert_eq!(receiving_pid(0, || Some(-1)), Some(0));
    }

    #[test]
    fn reads_explicit_unicode_string() {
        let ev = make_key_event(17, Some("t"));
        // SAFETY: valid event.
        let info = unsafe { read_key_event(ev) };
        assert_eq!(info.keycode, 17);
        assert_eq!(info.chars, "t");
        assert_eq!(
            key_category(info.keycode, info.flags, &info.chars),
            KeyCategory::Text
        );
        release(ev);
    }

    #[test]
    fn reads_vietnamese_and_space() {
        for (code, text) in [(49u16, " "), (0, "ơ"), (0, "đ")] {
            let ev = make_key_event(code, Some(text));
            // SAFETY: valid event.
            let info = unsafe { read_key_event(ev) };
            assert_eq!(info.chars, text);
            release(ev);
        }
    }

    #[test]
    fn multi_char_unicode_is_not_truncated() {
        // Automation tools may put a whole word into one key event. The v0.1.0 reader used a
        // fixed 8-unit buffer and silently truncated longer strings.
        let long = "Hello everyone 👋 xin chào";
        let ev = make_key_event(0, Some(long));
        // SAFETY: valid event.
        let info = unsafe { read_key_event(ev) };
        assert_eq!(info.chars, long);
        release(ev);
    }

    #[test]
    fn marker_identifies_own_events_and_unposted_event_has_no_target() {
        let ev = make_key_event(17, Some("t"));
        // SAFETY: valid event.
        unsafe { CGEventSetIntegerValueField(ev, FIELD_SOURCE_USER_DATA, SYNTHETIC_MARKER) };
        // SAFETY: valid event.
        let info = unsafe { read_key_event(ev) };
        assert!(is_own_event(&info));
        assert_eq!(info.target_pid, 0, "unposted events carry no target pid");
        release(ev);
    }

    #[test]
    fn keycode_only_event_has_empty_unicode() {
        // Observed on this machine: an event built from a key code alone carries no Unicode
        // string. v0.1.0 treated such keys as a reset, so they could never complete a match.
        let ev = make_key_event(17, None);
        // SAFETY: valid event.
        let info = unsafe { read_key_event(ev) };
        assert_eq!(info.chars, "");
        assert_eq!(
            key_category(info.keycode, info.flags, &info.chars),
            KeyCategory::EmptyText
        );
        release(ev);
    }

    /// Captured, non-posted view of an emitted event.
    #[derive(Debug, PartialEq)]
    struct Captured {
        down: bool,
        keycode: u16,
        chars: String,
        own: bool,
        flags: u64,
    }

    fn capture(
        steps: &[InjectStep],
        gate: &dyn Fn() -> Result<(), AbortReason>,
    ) -> (Vec<Captured>, Result<(), AbortReason>) {
        let source = make_synthetic_source();
        assert!(!source.is_null());
        let mut out = Vec::new();
        let mut sink = |ev: CGEventRef| {
            // SAFETY: valid event for the duration of the sink call.
            let (info, ty) = unsafe { (read_key_event(ev), CGEventGetType(ev)) };
            out.push(Captured {
                down: ty == EV_KEY_DOWN,
                keycode: info.keycode,
                chars: info.chars.clone(),
                own: is_own_event(&info),
                flags: info.flags,
            });
        };
        let mut result = Ok(());
        for step in steps {
            result = post_step(source, step, gate, &mut sink);
            if result.is_err() {
                break;
            }
        }
        // SAFETY: we own the source.
        unsafe { CFRelease(source as CFTypeRef) };
        (out, result)
    }

    #[test]
    fn source_user_data_tags_events_without_per_event_field() {
        // Contract check for CGEventSourceSetUserData: an event created from the tagged
        // source carries the marker even if the per-event field is never set.
        let source = make_synthetic_source();
        // SAFETY: plain construction, released below.
        unsafe {
            assert_eq!(CGEventSourceGetUserData(source), SYNTHETIC_MARKER);
            let ev = CGEventCreateKeyboardEvent(source, 51, true);
            let info = read_key_event(ev);
            assert_eq!(info.marker, SYNTHETIC_MARKER);
            assert!(is_own_event(&info));
            // Private sources get a generated state ID, distinct from the combined-session (0)
            // and HID-system (1) states that real keyboards use.
            assert!(info.source_state_id != 0 && info.source_state_id != 1);
            CFRelease(ev as CFTypeRef);
            CFRelease(source as CFTypeRef);
        }
    }

    #[test]
    fn every_injected_event_is_recognised_as_own() {
        // Guards the hypothesised v0.1.1 Notes failure mode (unproven): injected keys must
        // never look like foreign input, which would cancel the injection mid-way.
        let steps = [
            InjectStep::Backspaces(5),
            InjectStep::Text("Hello everyone 2026-10-13 10:08".into()),
            InjectStep::Return,
            InjectStep::Text("xin chào".into()),
            InjectStep::Left(2),
        ];
        let (events, result) = capture(&steps, &|| Ok(()));
        assert_eq!(result, Ok(()));
        assert_eq!(events.len(), (5 + 1 + 1 + 1 + 2) * 2);
        assert!(events.iter().all(|e| e.own), "all synthetic events tagged");
        assert!(events
            .iter()
            .all(|e| e.flags & (super::super::FLAG_COMMAND | super::super::FLAG_CONTROL) == 0));
        // Down/up pairs in order.
        assert!(events.chunks(2).all(|p| p[0].down && !p[1].down));
        let downs: Vec<&Captured> = events.iter().filter(|e| e.down).collect();
        assert!(downs[..5]
            .iter()
            .all(|e| e.keycode == super::super::keycode::DELETE));
        assert_eq!(downs[5].chars, "Hello everyone 2026-10-13 10:08");
        assert_eq!(downs[6].keycode, super::super::keycode::RETURN);
        assert_eq!(downs[7].chars, "xin chào");
        assert!(downs[8..]
            .iter()
            .all(|e| e.keycode == super::super::keycode::LEFT));
    }

    #[test]
    fn gate_failure_stops_before_next_key() {
        use std::cell::Cell;
        let calls = Cell::new(0);
        let gate = || {
            calls.set(calls.get() + 1);
            if calls.get() > 2 {
                Err(AbortReason::InputChanged)
            } else {
                Ok(())
            }
        };
        let (events, result) = capture(&[InjectStep::Backspaces(5)], &gate);
        assert_eq!(result, Err(AbortReason::InputChanged));
        assert_eq!(
            events.len(),
            4,
            "exactly two keys (down+up) before the gate failed"
        );
    }

    #[test]
    fn focus_flip_mid_text_stops_before_the_next_event() {
        use std::cell::Cell;
        // A long multi-chunk insertion; the "focused field" changes after the 3rd check.
        let rep = crate::engine::Replacement {
            delete_chars: 0,
            delete_text: String::new(),
            insert: "x".repeat(super::super::MAX_UTF16_PER_EVENT * 5),
            after_cursor: String::new(),
            warnings: vec![],
        };
        let steps = plan_injection(&rep);
        assert_eq!(steps.len(), 5, "five text chunks");
        let checks = Cell::new(0);
        let gate = || {
            checks.set(checks.get() + 1);
            if checks.get() > 3 {
                Err(AbortReason::FocusChanged)
            } else {
                Ok(())
            }
        };
        let source = make_synthetic_source();
        let mut events = 0;
        let mut sink = |_ev: CGEventRef| events += 1;
        let result = run_steps(source, &steps, &gate, &mut sink);
        // SAFETY: we own the source.
        unsafe { CFRelease(source as CFTypeRef) };
        assert_eq!(result, Err((AbortReason::FocusChanged, true)));
        assert_eq!(
            events,
            3 * 2,
            "three chunks (down+up), nothing after the flip"
        );
        assert_eq!(checks.get(), 4, "checked before every chunk");

        // A flip before anything was sent reports no partial injection.
        let never = || Err(AbortReason::FocusChanged);
        let source = make_synthetic_source();
        let mut none = 0;
        let mut sink = |_ev: CGEventRef| none += 1;
        let result = run_steps(source, &steps, &never, &mut sink);
        // SAFETY: we own the source.
        unsafe { CFRelease(source as CFTypeRef) };
        assert_eq!(result, Err((AbortReason::FocusChanged, false)));
        assert_eq!(none, 0);
    }

    #[test]
    fn cancel_metadata_has_no_content() {
        let info = KeyEventInfo {
            marker: 0,
            source_state_id: 1,
            source_pid: 4242,
            target_pid: 0,
            keycode: 17,
            flags: 0,
            chars: "secret".into(),
        };
        let note = describe_cancel_event(&info, KeyCategory::Text, 12);
        assert!(note.contains("source pid 4242"));
        assert!(note.contains("marker match: false"));
        assert!(note.contains("+12 ms"));
        assert!(!note.contains("secret") && !note.contains("17"));
    }

    #[test]
    fn carbon_modifiers_mapping() {
        assert_eq!(carbon_modifier_state(0), 0);
        assert_eq!(carbon_modifier_state(0x0002_0000), 0x02); // shift
        assert_eq!(carbon_modifier_state(0x0008_0000), 0x08); // option
        assert_eq!(carbon_modifier_state(0x0010_0000), 0x01); // command
        assert_eq!(
            carbon_modifier_state(0x0001_0000 | 0x0004_0000),
            0x04 | 0x10
        );
    }

    #[test]
    fn own_process_targets_are_skipped_but_others_are_not() {
        let me = std::process::id();
        assert!(targets_own_process(me as i64, me));
        assert!(
            !targets_own_process(me as i64 + 1, me),
            "other apps stay monitored"
        );
        assert!(
            !targets_own_process(0, me),
            "unknown target is not treated as ours"
        );
        assert!(!targets_own_process(-1, me));
    }

    #[test]
    fn foreign_source_pid_is_not_treated_as_own() {
        let info = KeyEventInfo {
            marker: 0,
            source_state_id: 1,
            source_pid: std::process::id() as i64 + 1,
            target_pid: 0,
            keycode: 17,
            flags: 0,
            chars: "t".into(),
        };
        assert!(!is_own_event(&info));
        let mine = KeyEventInfo {
            source_pid: std::process::id() as i64,
            ..info
        };
        assert!(is_own_event(&mine));
    }
}
