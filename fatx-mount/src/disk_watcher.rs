//! Disk removal detection for macOS.
//!
//! Detects when a USB drive is physically disconnected (yanked) or safely
//! ejected, so the NFS server can shut down gracefully and avoid the
//! catastrophic stale mount deadlock.
//!
//! Two detection layers run concurrently:
//!
//! 1. **IOKit device termination** (primary — works for yanks): Registers an
//!    `IOServiceAddInterestNotification` on the IOMedia service matching the
//!    BSD disk name. When the USB device is physically removed, the kernel
//!    fires `kIOMessageServiceIsTerminated` within ~1-5ms. This is the same
//!    pattern used by Hammerspoon, node-usb-detection, and Apple's own USB
//!    notification examples.
//!
//! 2. **DiskArbitration** (backup — works for safe ejects): Registers a
//!    `DADiskDisappearedCallback`. Fires when the disk object is cleaned up
//!    from the DA registry. Unreliable for yanks (fires on replug, not
//!    unplug) but handles Finder "Eject" properly.

#[allow(unused_imports)]
use log::{info, warn};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Opaque handle to a running disk watcher. Dropping it stops the watcher.
pub struct DiskWatcher {
    /// When set to true, the watcher threads will exit.
    stop: Arc<AtomicBool>,
    /// Whether the disk has disappeared.
    pub disappeared: Arc<AtomicBool>,
}

impl DiskWatcher {
    /// Start watching a BSD disk name (e.g. "disk4") for removal.
    ///
    /// The `bsd_name` should be the base disk name without the `/dev/` prefix
    /// and without partition suffixes (e.g. "disk4" not "disk4s1" or "/dev/rdisk4").
    ///
    /// Returns a `DiskWatcher` handle. Check `watcher.disappeared` to see if the
    /// disk has been removed. The watcher runs until dropped.
    pub fn start(bsd_name: &str, _device_path: &str) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let disappeared = Arc::new(AtomicBool::new(false));

        #[cfg(target_os = "macos")]
        {
            // Start IOKit watcher thread (primary — fires immediately on yank)
            let iokit_stop = Arc::clone(&stop);
            let iokit_disappeared = Arc::clone(&disappeared);
            let iokit_bsd_name = bsd_name.to_string();
            std::thread::Builder::new()
                .name("iokit-device-watcher".into())
                .spawn(move || {
                    iokit_watcher_thread(&iokit_bsd_name, &iokit_stop, &iokit_disappeared);
                })
                .expect("failed to spawn IOKit watcher thread");

            // Start DiskArbitration watcher thread (backup — handles safe ejects)
            let da_stop = Arc::clone(&stop);
            let da_disappeared = Arc::clone(&disappeared);
            let da_bsd_name = bsd_name.to_string();
            std::thread::Builder::new()
                .name("disk-arbitration-watcher".into())
                .spawn(move || {
                    da_watcher_thread(&da_bsd_name, &da_stop, &da_disappeared);
                })
                .expect("failed to spawn DiskArbitration watcher thread");
        }

        info!("Disk watcher started for {}", bsd_name);

        DiskWatcher { stop, disappeared }
    }

    /// Check if the disk has disappeared.
    #[allow(dead_code)]
    pub fn is_disappeared(&self) -> bool {
        self.disappeared.load(Ordering::Relaxed)
    }
}

impl Drop for DiskWatcher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// Extract the BSD disk name from a device path.
///
/// E.g. "/dev/rdisk4" → "disk4", "/dev/disk4s1" → "disk4"
pub fn bsd_name_from_device_path(device_path: &str) -> Option<String> {
    let basename = device_path.rsplit('/').next()?;
    // Strip leading 'r' for raw devices: "rdisk4" → "disk4"
    let name = if basename.starts_with('r') && basename[1..].starts_with("disk") {
        &basename[1..]
    } else {
        basename
    };
    // Strip partition suffix: "disk4s1" → "disk4"
    if let Some(after_disk) = name.strip_prefix("disk") {
        let digit_end = after_disk
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(after_disk.len());
        if digit_end > 0 {
            return Some(format!("disk{}", &after_disk[..digit_end]));
        }
    }
    Some(name.to_string())
}

// ── IOKit device termination watcher (macOS only) ─────────────────────────
//
// Registers an interest notification on the IOMedia service matching our BSD
// disk name. When the physical USB device is removed, the kernel fires
// kIOMessageServiceIsTerminated on the service, which our callback catches.
//
// This is lower-level than DiskArbitration — it fires at the kernel driver
// level, not the userspace disk daemon level. That's why it works for yanks.

#[cfg(target_os = "macos")]
fn iokit_watcher_thread(bsd_name: &str, stop: &AtomicBool, disappeared: &AtomicBool) {
    use core_foundation::base::TCFType;
    use core_foundation::runloop::{kCFRunLoopDefaultMode, CFRunLoop};
    use core_foundation::string::CFString;
    use core_foundation_sys::base::{kCFAllocatorDefault, CFTypeRef};
    use core_foundation_sys::string::CFStringRef;
    use io_kit_sys::types::{io_iterator_t, io_object_t};
    use io_kit_sys::*;
    use log::error;
    use mach2::kern_return::KERN_SUCCESS;
    use std::ffi::c_void;

    // Additional IOKit functions not in io-kit-sys
    #[link(name = "IOKit", kind = "framework")]
    extern "C" {
        fn IORegistryEntryCreateCFProperty(
            entry: io_object_t,
            key: CFStringRef,
            allocator: *const c_void,
            options: u32,
        ) -> CFTypeRef;
    }

    // IOKit termination notification type string
    const K_IO_TERMINATED_NOTIFICATION: &std::ffi::CStr = c"IOServiceTerminate";

    // Context passed to the IOKit callback
    struct IoKitContext {
        disappeared: *const AtomicBool,
        target_bsd_name: String,
    }
    unsafe impl Send for IoKitContext {}
    unsafe impl Sync for IoKitContext {}

    // Wrapper to make *mut IoKitContext Send+Sync for the static Mutex.
    // Safety: access is serialized by the Mutex and the pointer is only
    // dereferenced inside the IOKit callback on the run-loop thread.
    struct CtxPtr(*mut IoKitContext);
    unsafe impl Send for CtxPtr {}
    unsafe impl Sync for CtxPtr {}

    // Shared context — must be static for the callback
    static IOKIT_CTX: std::sync::Mutex<Option<CtxPtr>> = std::sync::Mutex::new(None);

    /// Drain an IOKit iterator, checking each terminated service.
    /// If any matches our target BSD name, signal removal.
    /// IMPORTANT: You MUST drain the iterator to re-arm the notification.
    unsafe fn drain_terminated_iterator(iterator: io_iterator_t) {
        let ctx_guard = IOKIT_CTX.lock().unwrap();
        let ctx_ptr = match ctx_guard.as_ref() {
            Some(p) => p.0,
            None => return,
        };
        let ctx = &*ctx_ptr;

        let bsd_key = CFString::new("BSD Name");

        loop {
            let service = IOIteratorNext(iterator);
            if service == 0 {
                break;
            }

            // Try to read the "BSD Name" property from this terminated service
            let prop = IORegistryEntryCreateCFProperty(
                service,
                bsd_key.as_concrete_TypeRef(),
                kCFAllocatorDefault,
                0,
            );

            if !prop.is_null() {
                // Convert CFString to Rust string
                let cf_str: core_foundation::string::CFString =
                    core_foundation::base::TCFType::wrap_under_create_rule(prop as CFStringRef);
                let name = cf_str.to_string();

                eprintln!(
                    "[IOKit] Terminated service: BSD Name='{}' (handle=0x{:x})",
                    name, service
                );

                if name == ctx.target_bsd_name {
                    eprintln!(
                        "[IOKit] *** OUR DEVICE '{}' TERMINATED — triggering shutdown ***",
                        name
                    );
                    (*ctx.disappeared).store(true, Ordering::Relaxed);
                }
            } else {
                eprintln!(
                    "[IOKit] Terminated service 0x{:x} (no BSD Name property — not a disk)",
                    service
                );
            }

            IOObjectRelease(service);
        }
    }

    /// Callback fired when IOMedia services are terminated.
    /// The iterator contains the newly terminated services — we MUST drain it
    /// to re-arm the notification for future events.
    unsafe extern "C" fn media_terminated_callback(_refcon: *mut c_void, iterator: io_iterator_t) {
        eprintln!("[IOKit CALLBACK] IOMedia termination notification fired!");
        drain_terminated_iterator(iterator);
    }

    /// Callback fired when IOUSBHostDevice services are terminated.
    unsafe extern "C" fn usb_terminated_callback(_refcon: *mut c_void, iterator: io_iterator_t) {
        eprintln!("[IOKit CALLBACK] IOUSBHostDevice termination notification fired!");

        // Drain the iterator (re-arms notification). USB devices don't have
        // BSD Name, so just check if anything was terminated.
        let ctx_guard = IOKIT_CTX.lock().unwrap();
        let ctx_ptr = match ctx_guard.as_ref() {
            Some(p) => p.0,
            None => {
                // Still drain to re-arm
                loop {
                    let s = IOIteratorNext(iterator);
                    if s == 0 {
                        break;
                    }
                    IOObjectRelease(s);
                }
                return;
            }
        };
        let ctx = &*ctx_ptr;

        let mut count = 0u32;
        loop {
            let service = IOIteratorNext(iterator);
            if service == 0 {
                break;
            }
            count += 1;
            eprintln!("[IOKit] Terminated USB device: handle=0x{:x}", service);
            IOObjectRelease(service);
        }

        if count > 0 {
            // A USB device was terminated. We can't easily match it to our
            // specific disk, but if we also get an IOMedia termination for
            // our BSD name, we're covered. For now, log it.
            // If the IOMedia callback doesn't fire (zombie), this is our
            // only signal. Signal removal if we see USB termination.
            eprintln!(
                "[IOKit] *** USB device terminated ({} device(s)) — triggering shutdown ***",
                count
            );
            (*ctx.disappeared).store(true, Ordering::Relaxed);
        }
    }

    info!("[IOKit] Starting device watcher for BSD name: {}", bsd_name);

    unsafe {
        // Create the context and store in static
        let ctx = Box::new(IoKitContext {
            disappeared: disappeared as *const AtomicBool,
            target_bsd_name: bsd_name.to_string(),
        });
        let ctx_ptr = Box::into_raw(ctx);
        *IOKIT_CTX.lock().unwrap() = Some(CtxPtr(ctx_ptr));

        // Create a notification port and wire it to this thread's run loop
        let notify_port = IONotificationPortCreate(kIOMasterPortDefault);
        if notify_port.is_null() {
            error!("[IOKit] Failed to create notification port");
            *IOKIT_CTX.lock().unwrap() = None;
            drop(Box::from_raw(ctx_ptr));
            return;
        }
        let run_loop_source = IONotificationPortGetRunLoopSource(notify_port);
        let run_loop = CFRunLoop::get_current();
        core_foundation_sys::runloop::CFRunLoopAddSource(
            run_loop.as_concrete_TypeRef() as *mut _,
            run_loop_source,
            kCFRunLoopDefaultMode,
        );

        // ── Register for IOMedia termination ──
        // Watch for ANY IOMedia service being terminated. In the callback,
        // we check if the BSD Name matches ours.
        let media_matching = IOServiceMatching(c"IOMedia".as_ptr());
        let mut media_iterator: io_iterator_t = 0;
        let kr = IOServiceAddMatchingNotification(
            notify_port,
            K_IO_TERMINATED_NOTIFICATION.as_ptr() as *const _,
            media_matching as _,
            media_terminated_callback,
            ctx_ptr as *mut c_void,
            &mut media_iterator,
        );
        if kr != KERN_SUCCESS {
            error!("[IOKit] Failed to register IOMedia termination: 0x{:x}", kr);
        } else {
            info!("[IOKit] Registered IOMedia termination watcher");
            // CRITICAL: Drain the iterator to arm the notification.
            // Without this, the callback will never fire.
            drain_terminated_iterator(media_iterator);
        }

        // ── Register for IOUSBHostDevice termination ──
        // Watch for USB device removal at the host level.
        let usb_matching = IOServiceMatching(c"IOUSBHostDevice".as_ptr());
        let mut usb_iterator: io_iterator_t = 0;
        let kr = IOServiceAddMatchingNotification(
            notify_port,
            K_IO_TERMINATED_NOTIFICATION.as_ptr() as *const _,
            usb_matching as _,
            usb_terminated_callback,
            ctx_ptr as *mut c_void,
            &mut usb_iterator,
        );
        if kr != KERN_SUCCESS {
            warn!(
                "[IOKit] Failed to register IOUSBHostDevice termination: 0x{:x} (non-fatal)",
                kr
            );
        } else {
            info!("[IOKit] Registered IOUSBHostDevice termination watcher");
            // Drain to arm
            loop {
                let s = IOIteratorNext(usb_iterator);
                if s == 0 {
                    break;
                }
                IOObjectRelease(s);
            }
        }

        info!("[IOKit] All watchers armed, entering run loop");

        // Run the loop
        let mut tick: u64 = 0;
        while !stop.load(Ordering::Relaxed) && !disappeared.load(Ordering::Relaxed) {
            let result =
                core_foundation_sys::runloop::CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.5, 0);
            tick += 1;
            if tick.is_multiple_of(30) {
                info!(
                    "[IOKit] Run loop heartbeat (tick={}, result={})",
                    tick, result
                );
            }
        }

        if disappeared.load(Ordering::Relaxed) {
            info!("[IOKit] Device removal detected, stopping");
        } else {
            info!("[IOKit] Watcher stopping (stop flag set)");
        }

        // Cleanup
        if media_iterator != 0 {
            IOObjectRelease(media_iterator);
        }
        if usb_iterator != 0 {
            IOObjectRelease(usb_iterator);
        }
        IONotificationPortDestroy(notify_port);
        *IOKIT_CTX.lock().unwrap() = None;
        drop(Box::from_raw(ctx_ptr));
    }
}

// ── DiskArbitration watcher (macOS only) ──────────────────────────────────
//
// Backup detection for safe ejects. Fires when the disk object is removed
// from the DA registry. Unreliable for USB yanks (fires on replug) but
// handles "Eject" from Finder properly.

#[cfg(target_os = "macos")]
fn da_watcher_thread(bsd_name: &str, stop: &AtomicBool, disappeared: &AtomicBool) {
    use core_foundation::base::TCFType;
    use core_foundation::dictionary::CFMutableDictionary;
    use core_foundation::runloop::{kCFRunLoopDefaultMode, CFRunLoop};
    use core_foundation::string::CFString;
    use log::error;
    use std::ffi::c_void;

    #[link(name = "DiskArbitration", kind = "framework")]
    extern "C" {
        fn DASessionCreate(allocator: *const c_void) -> *mut c_void;
        fn DASessionScheduleWithRunLoop(
            session: *mut c_void,
            run_loop: *mut c_void,
            mode: *const c_void,
        );
        fn DASessionUnscheduleFromRunLoop(
            session: *mut c_void,
            run_loop: *mut c_void,
            mode: *const c_void,
        );
        fn DARegisterDiskDisappearedCallback(
            session: *mut c_void,
            match_dict: *const c_void,
            callback: extern "C" fn(*mut c_void, *mut c_void),
            context: *mut c_void,
        );
        fn DAUnregisterCallback(
            session: *mut c_void,
            callback: *const c_void,
            context: *mut c_void,
        );
    }

    extern "C" {
        static kCFAllocatorDefault: *const c_void;
        static kDADiskDescriptionMediaBSDNameKey: *const c_void;
    }

    struct CallbackContext {
        disappeared: *const AtomicBool,
    }
    unsafe impl Send for CallbackContext {}

    extern "C" fn disk_disappeared_callback(_disk: *mut c_void, context: *mut c_void) {
        let ctx = unsafe { &*(context as *const CallbackContext) };
        let disappeared = unsafe { &*ctx.disappeared };
        warn!("[DiskArbitration] Disk disappeared callback fired.");
        disappeared.store(true, Ordering::Relaxed);
    }

    info!(
        "[DiskArbitration] Starting watcher for BSD name: {}",
        bsd_name
    );

    unsafe {
        let session = DASessionCreate(kCFAllocatorDefault);
        if session.is_null() {
            error!("[DiskArbitration] Failed to create DA session");
            return;
        }

        let mut match_dict = CFMutableDictionary::new();
        let bsd_cf = CFString::new(bsd_name);
        match_dict.add(&kDADiskDescriptionMediaBSDNameKey, &bsd_cf.as_CFTypeRef());

        let ctx = Box::new(CallbackContext {
            disappeared: disappeared as *const AtomicBool,
        });
        let ctx_ptr = Box::into_raw(ctx) as *mut c_void;

        DARegisterDiskDisappearedCallback(
            session,
            match_dict.as_concrete_TypeRef() as *const c_void,
            disk_disappeared_callback,
            ctx_ptr,
        );

        let run_loop = CFRunLoop::get_current();
        DASessionScheduleWithRunLoop(
            session,
            run_loop.as_concrete_TypeRef() as *mut c_void,
            kCFRunLoopDefaultMode as *mut c_void,
        );

        info!("[DiskArbitration] Watcher registered, entering run loop");

        while !stop.load(Ordering::Relaxed) && !disappeared.load(Ordering::Relaxed) {
            core_foundation_sys::runloop::CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.5, 0);
        }

        info!("[DiskArbitration] Watcher stopping");

        DAUnregisterCallback(session, disk_disappeared_callback as *const c_void, ctx_ptr);
        DASessionUnscheduleFromRunLoop(
            session,
            run_loop.as_concrete_TypeRef() as *mut c_void,
            kCFRunLoopDefaultMode as *mut c_void,
        );
        drop(Box::from_raw(ctx_ptr as *mut CallbackContext));
        core_foundation_sys::base::CFRelease(session as *const c_void);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bsd_name_from_rdisk() {
        assert_eq!(
            bsd_name_from_device_path("/dev/rdisk4"),
            Some("disk4".to_string())
        );
    }

    #[test]
    fn test_bsd_name_from_disk() {
        assert_eq!(
            bsd_name_from_device_path("/dev/disk4"),
            Some("disk4".to_string())
        );
    }

    #[test]
    fn test_bsd_name_from_partition() {
        assert_eq!(
            bsd_name_from_device_path("/dev/disk4s1"),
            Some("disk4".to_string())
        );
    }

    #[test]
    fn test_bsd_name_from_raw_partition() {
        assert_eq!(
            bsd_name_from_device_path("/dev/rdisk4s2"),
            Some("disk4".to_string())
        );
    }

    #[test]
    fn test_bsd_name_from_image() {
        assert_eq!(
            bsd_name_from_device_path("/tmp/test.img"),
            Some("test.img".to_string())
        );
    }
}
