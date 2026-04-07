//! Platform-specific helpers for device access.
//!
//! On macOS, raw block devices (/dev/rdiskN) don't support `seek(End(0))`
//! to determine size. We use ioctl to query disk geometry instead.

#[allow(unused_imports)]
use log::debug;

/// Get the size of a block device via macOS ioctls.
/// Returns None if the ioctls fail (e.g., not a block device, or not on macOS).
#[cfg(target_os = "macos")]
pub fn get_block_device_size(fd: std::os::unix::io::RawFd) -> Option<u64> {
    // DKIOCGETBLOCKSIZE  = _IOR('d', 24, uint32_t) = 0x40046418
    // DKIOCGETBLOCKCOUNT = _IOR('d', 25, uint64_t) = 0x40086419
    const DKIOCGETBLOCKSIZE: libc::c_ulong = 0x40046418;
    const DKIOCGETBLOCKCOUNT: libc::c_ulong = 0x40086419;

    let mut block_size: u32 = 0;
    let mut block_count: u64 = 0;

    unsafe {
        if libc::ioctl(fd, DKIOCGETBLOCKSIZE, &mut block_size) != 0 {
            debug!(
                "DKIOCGETBLOCKSIZE ioctl failed: {}",
                std::io::Error::last_os_error()
            );
            return None;
        }
        if libc::ioctl(fd, DKIOCGETBLOCKCOUNT, &mut block_count) != 0 {
            debug!(
                "DKIOCGETBLOCKCOUNT ioctl failed: {}",
                std::io::Error::last_os_error()
            );
            return None;
        }
    }

    debug!(
        "ioctl: block_size={}, block_count={}, total={} bytes",
        block_size,
        block_count,
        block_size as u64 * block_count
    );
    Some(block_size as u64 * block_count)
}

/// Stub for non-macOS platforms.
#[cfg(not(target_os = "macos"))]
pub fn get_block_device_size(_fd: i32) -> Option<u64> {
    None
}
