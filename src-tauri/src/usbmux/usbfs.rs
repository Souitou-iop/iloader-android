//! [`UsbPipe`](super::pipe::UsbPipe) on top of the Linux `usbfs` interface.
//!
//! Android hands an app an open file descriptor for `/dev/bus/usb/BBB/DDD`
//! (`UsbDeviceConnection.getFileDescriptor()`). Bulk transfers on it are plain
//! ioctls, so no libusb is needed. Only the ioctl numbers and structure
//! layouts below are platform independent; the pipe itself is Linux only.

use std::ffi::c_void;
use std::mem::size_of;

const IOC_NONE: u32 = 0;
const IOC_WRITE: u32 = 1;
const IOC_READ: u32 = 2;

/// The generic `_IOC` encoding used by arm, arm64, x86 and x86_64.
const fn ioc(dir: u32, nr: u32, size: usize) -> u32 {
    (dir << 30) | ((size as u32) << 16) | ((b'U' as u32) << 8) | nr
}

#[repr(C)]
pub struct BulkTransfer {
    pub ep: u32,
    pub len: u32,
    pub timeout: u32,
    pub data: *mut c_void,
}

#[repr(C)]
pub struct Urb {
    pub type_: u8,
    pub endpoint: u8,
    pub status: i32,
    pub flags: u32,
    pub buffer: *mut c_void,
    pub buffer_length: i32,
    pub actual_length: i32,
    pub start_frame: i32,
    pub stream_id: u32,
    pub error_count: i32,
    pub signr: u32,
    pub usercontext: *mut c_void,
}

pub const URB_TYPE_BULK: u8 = 3;

pub const USBDEVFS_BULK: u32 = ioc(IOC_READ | IOC_WRITE, 2, size_of::<BulkTransfer>());
pub const USBDEVFS_SUBMITURB: u32 = ioc(IOC_READ, 10, size_of::<Urb>());
pub const USBDEVFS_DISCARDURB: u32 = ioc(IOC_NONE, 11, 0);
pub const USBDEVFS_REAPURB: u32 = ioc(IOC_WRITE, 12, size_of::<*mut c_void>());
pub const USBDEVFS_REAPURBNDELAY: u32 = ioc(IOC_WRITE, 13, size_of::<*mut c_void>());

/// Largest synchronous bulk transfer older kernels accept.
pub const MAX_SYNC_BULK: usize = 16384;

#[cfg(any(target_os = "linux", target_os = "android"))]
pub use pipe::UsbfsPipe;

#[cfg(any(target_os = "linux", target_os = "android"))]
mod pipe {
    use std::io;
    use std::os::fd::{AsRawFd, OwnedFd};
    use std::sync::Mutex;
    use std::time::Duration;

    use super::*;
    use crate::usbmux::packet::USB_MRU;
    use crate::usbmux::pipe::UsbPipe;

    const WRITE_TIMEOUT_MS: u32 = 5000;

    /// An asynchronous bulk-in request that stays in flight between calls to
    /// `read`, so a timeout never loses data.
    struct Rx {
        /// Boxed so the kernel can keep pointers to them.
        urb: Box<Urb>,
        buf: Box<[u8]>,
        submitted: bool,
    }

    pub struct UsbfsPipe {
        fd: OwnedFd,
        ep_in: u8,
        ep_out: u8,
        max_packet_out: usize,
        rx: Mutex<Rx>,
    }

    // The raw pointers inside `Urb` only ever point into `Rx::buf`.
    unsafe impl Send for Rx {}

    impl UsbfsPipe {
        /// `fd` must be a usbfs descriptor whose usbmux interface has been
        /// claimed.
        pub fn new(fd: OwnedFd, ep_in: u8, ep_out: u8, max_packet_out: usize) -> Self {
            UsbfsPipe {
                fd,
                ep_in,
                ep_out,
                max_packet_out: max_packet_out.max(1),
                rx: Mutex::new(Rx {
                    urb: Box::new(Urb {
                        type_: URB_TYPE_BULK,
                        endpoint: 0,
                        status: 0,
                        flags: 0,
                        buffer: std::ptr::null_mut(),
                        buffer_length: 0,
                        actual_length: 0,
                        start_frame: 0,
                        stream_id: 0,
                        error_count: 0,
                        signr: 0,
                        usercontext: std::ptr::null_mut(),
                    }),
                    buf: vec![0u8; USB_MRU].into_boxed_slice(),
                    submitted: false,
                }),
            }
        }

        fn ioctl(&self, request: u32, arg: *mut c_void) -> io::Result<i32> {
            loop {
                // SAFETY: `arg` points to a live structure of the type the
                // request expects, supplied by the callers in this file.
                let res = unsafe { libc::ioctl(self.fd.as_raw_fd(), request as _, arg) };
                if res >= 0 {
                    return Ok(res);
                }
                let err = io::Error::last_os_error();
                if err.kind() != io::ErrorKind::Interrupted {
                    return Err(err);
                }
            }
        }

        fn bulk_out(&self, data: &[u8]) -> io::Result<()> {
            let mut sent = 0;
            // A zero length packet is a transfer of its own, so run the loop
            // once for empty input.
            loop {
                let chunk = &data[sent..data.len().min(sent + MAX_SYNC_BULK)];
                let mut transfer = BulkTransfer {
                    ep: u32::from(self.ep_out),
                    len: chunk.len() as u32,
                    timeout: WRITE_TIMEOUT_MS,
                    data: chunk.as_ptr() as *mut c_void,
                };
                let n = self.ioctl(USBDEVFS_BULK, &mut transfer as *mut _ as *mut c_void)? as usize;
                sent += n;
                if sent >= data.len() {
                    return Ok(());
                }
                if n == 0 {
                    return Err(io::ErrorKind::WriteZero.into());
                }
            }
        }
    }

    impl UsbPipe for UsbfsPipe {
        fn read(&self, out: &mut [u8], timeout: Duration) -> io::Result<usize> {
            let mut rx = self.rx.lock().unwrap();
            if !rx.submitted {
                let len = out.len().min(rx.buf.len());
                rx.urb.endpoint = self.ep_in;
                rx.urb.buffer = rx.buf.as_mut_ptr() as *mut c_void;
                rx.urb.buffer_length = len as i32;
                rx.urb.actual_length = 0;
                rx.urb.status = 0;
                let urb: *mut Urb = &mut *rx.urb;
                self.ioctl(USBDEVFS_SUBMITURB, urb as *mut c_void)?;
                rx.submitted = true;
            }

            // usbfs signals a finished URB as POLLOUT.
            let mut pfd = libc::pollfd {
                fd: self.fd.as_raw_fd(),
                events: libc::POLLOUT,
                revents: 0,
            };
            let ms = timeout.as_millis().min(i32::MAX as u128) as i32;
            // SAFETY: `pfd` is a valid pollfd and we pass a count of one.
            let ready = unsafe { libc::poll(&mut pfd, 1, ms) };
            if ready < 0 {
                let err = io::Error::last_os_error();
                return Err(if err.kind() == io::ErrorKind::Interrupted {
                    io::ErrorKind::TimedOut.into()
                } else {
                    err
                });
            }
            if ready == 0 {
                return Err(io::ErrorKind::TimedOut.into());
            }
            if pfd.revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0
                && pfd.revents & libc::POLLOUT == 0
            {
                return Err(io::Error::from_raw_os_error(libc::ENODEV));
            }

            let mut reaped: *mut Urb = std::ptr::null_mut();
            if let Err(err) =
                self.ioctl(USBDEVFS_REAPURBNDELAY, &mut reaped as *mut _ as *mut c_void)
            {
                return Err(if err.raw_os_error() == Some(libc::EAGAIN) {
                    io::ErrorKind::TimedOut.into()
                } else {
                    err
                });
            }
            rx.submitted = false;
            if rx.urb.status != 0 {
                return Err(io::Error::from_raw_os_error(-rx.urb.status));
            }
            let n = (rx.urb.actual_length.max(0) as usize).min(out.len());
            out[..n].copy_from_slice(&rx.buf[..n]);
            Ok(n)
        }

        fn write(&self, data: &[u8]) -> io::Result<()> {
            self.bulk_out(data)?;
            // The device only sees the end of a transfer when a short packet
            // arrives, so a transfer that is a multiple of the packet size
            // has to be followed by an empty one.
            if data.len() % self.max_packet_out == 0 {
                self.bulk_out(&[])?;
            }
            Ok(())
        }
    }

    impl Drop for UsbfsPipe {
        fn drop(&mut self) {
            let rx = self.rx.get_mut().unwrap();
            if rx.submitted {
                let urb: *mut Urb = &mut *rx.urb;
                let _ = self.ioctl(USBDEVFS_DISCARDURB, urb as *mut c_void);
                // The kernel completes the discarded URB asynchronously;
                // reap it before the buffer is freed.
                let mut reaped: *mut Urb = std::ptr::null_mut();
                let _ = self.ioctl(USBDEVFS_REAPURB, &mut reaped as *mut _ as *mut c_void);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(target_pointer_width = "64")]
    fn ioctl_numbers_match_linux_usbdevice_fs_h_on_64_bit() {
        assert_eq!(size_of::<BulkTransfer>(), 24);
        assert_eq!(size_of::<Urb>(), 56);
        assert_eq!(USBDEVFS_BULK, 0xc018_5502);
        assert_eq!(USBDEVFS_SUBMITURB, 0x8038_550a);
        assert_eq!(USBDEVFS_DISCARDURB, 0x0000_550b);
        assert_eq!(USBDEVFS_REAPURB, 0x4008_550c);
        assert_eq!(USBDEVFS_REAPURBNDELAY, 0x4008_550d);
    }

    #[test]
    #[cfg(target_pointer_width = "32")]
    fn ioctl_numbers_match_linux_usbdevice_fs_h_on_32_bit() {
        assert_eq!(size_of::<BulkTransfer>(), 16);
        assert_eq!(size_of::<Urb>(), 44);
        assert_eq!(USBDEVFS_BULK, 0xc010_5502);
        assert_eq!(USBDEVFS_SUBMITURB, 0x802c_550a);
        assert_eq!(USBDEVFS_REAPURBNDELAY, 0x4004_550d);
    }
}
