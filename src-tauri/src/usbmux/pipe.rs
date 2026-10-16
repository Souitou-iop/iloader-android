use std::{io, time::Duration};

/// The bulk endpoint pair of a device's usbmux interface.
///
/// One reader and one writer thread use a pipe concurrently, so the methods
/// take `&self`.
pub trait UsbPipe: Send + Sync + 'static {
    /// Waits up to `timeout` for one bulk-in transfer and copies it into
    /// `buf`, which is at least `USB_MRU` bytes long. Returns
    /// `ErrorKind::TimedOut` when nothing arrived; any other error means the
    /// device is gone.
    fn read(&self, buf: &mut [u8], timeout: Duration) -> io::Result<usize>;

    /// Sends one mux packet as a single bulk-out transfer (a zero length
    /// packet is added when the transfer size requires one).
    fn write(&self, data: &[u8]) -> io::Result<()>;
}
