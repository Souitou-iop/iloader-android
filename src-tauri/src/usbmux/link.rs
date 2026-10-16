//! Drives a [`Session`] with a real [`UsbPipe`] and exposes its connections
//! as tokio streams.
//!
//! The session is plain data behind a mutex. A reader thread feeds it
//! bulk-in transfers, a writer thread sends what it queues, and streams
//! lock it directly. Wakers are registered under the same lock the reader
//! uses to dispatch events, so a wakeup can never be missed.

use std::collections::HashMap;
use std::io;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::task::{Context, Poll, Waker};
use std::time::{Duration, Instant};

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::{mpsc, oneshot};
use tracing::{debug, info, warn};

use super::packet::USB_MRU;
use super::pipe::UsbPipe;
use super::{CloseReason, ConnId, DeviceLevel, Event, MuxError, Session};

/// Longest the reader sleeps when no acknowledgement is pending; also how
/// quickly it notices a shutdown.
const IDLE_POLL: Duration = Duration::from_millis(200);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, thiserror::Error)]
pub enum ConnectError {
    #[error("the device refused the connection")]
    Refused,
    #[error("timed out waiting for the device")]
    Timeout,
    #[error("the device is gone")]
    DeviceGone,
    #[error(transparent)]
    Mux(#[from] MuxError),
}

#[derive(Default)]
struct Wait {
    read: Option<Waker>,
    write: Option<Waker>,
    connect: Option<oneshot::Sender<Result<(), ConnectError>>>,
}

struct State {
    session: Session,
    waits: HashMap<u16, Wait>,
    ready: Option<oneshot::Sender<Result<u8, MuxError>>>,
    gone: bool,
}

pub struct Link {
    state: Mutex<State>,
    tx: mpsc::UnboundedSender<Vec<u8>>,
    pipe: Arc<dyn UsbPipe>,
    epoch: Instant,
    stop: AtomicBool,
    on_gone: Mutex<Option<Box<dyn FnOnce() + Send>>>,
}

impl Link {
    /// Starts the I/O threads and sends the version request. The receiver
    /// resolves with the negotiated version once the device answered.
    /// `on_gone` runs once, if the device disappears on its own.
    pub fn start(
        pipe: Arc<dyn UsbPipe>,
        on_gone: impl FnOnce() + Send + 'static,
    ) -> (Arc<Link>, oneshot::Receiver<Result<u8, MuxError>>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let (ready_tx, ready_rx) = oneshot::channel();
        let link = Arc::new(Link {
            state: Mutex::new(State {
                session: Session::new(0),
                waits: HashMap::new(),
                ready: Some(ready_tx),
                gone: false,
            }),
            tx,
            pipe,
            epoch: Instant::now(),
            stop: AtomicBool::new(false),
            on_gone: Mutex::new(Some(Box::new(on_gone))),
        });
        {
            let mut st = link.lock();
            link.pump(&mut st);
        }
        let reader = link.clone();
        std::thread::Builder::new()
            .name("usbmux-rx".into())
            .spawn(move || reader.read_loop())
            .expect("spawn usbmux reader");
        let writer = link.clone();
        std::thread::Builder::new()
            .name("usbmux-tx".into())
            .spawn(move || writer.write_loop(rx))
            .expect("spawn usbmux writer");
        (link, ready_rx)
    }

    /// Stops the I/O threads and fails every open stream. Idempotent.
    pub fn shutdown(&self) {
        self.terminate(false);
    }

    /// Opens a stream to a TCP port on the device.
    pub async fn connect(self: &Arc<Self>, port: u16) -> Result<DeviceStream, ConnectError> {
        let (tx, rx) = oneshot::channel();
        let id = {
            let mut st = self.lock();
            let id = st.session.connect(port)?;
            st.waits.entry(id.0).or_default().connect = Some(tx);
            self.pump(&mut st);
            id
        };
        // Frees the connection again unless it is handed out below.
        let mut stream = DeviceStream {
            link: self.clone(),
            id,
        };
        match tokio::time::timeout(CONNECT_TIMEOUT, rx).await {
            Ok(Ok(Ok(()))) => Ok(stream),
            Ok(Ok(Err(err))) => Err(err),
            Ok(Err(_)) => Err(ConnectError::DeviceGone),
            Err(_) => {
                stream.close();
                Err(ConnectError::Timeout)
            }
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn now_ms(&self) -> u64 {
        self.epoch.elapsed().as_millis() as u64
    }

    /// Moves everything the session wants to say to the outside world:
    /// packets to the writer thread, events to whoever waits for them.
    fn pump(&self, st: &mut State) {
        while let Some(frame) = st.session.poll_transmit() {
            let _ = self.tx.send(frame);
        }
        while let Some(event) = st.session.poll_event() {
            match event {
                Event::Ready { version, minor } => {
                    info!("usbmux protocol {version}.{minor} established");
                    if let Some(tx) = st.ready.take() {
                        let _ = tx.send(Ok(version));
                    }
                }
                Event::Connected(id) => {
                    if let Some(tx) = st.waits.entry(id.0).or_default().connect.take() {
                        let _ = tx.send(Ok(()));
                    }
                }
                Event::ConnectFailed(id) => {
                    if let Some(mut wait) = st.waits.remove(&id.0) {
                        if let Some(tx) = wait.connect.take() {
                            let _ = tx.send(Err(ConnectError::Refused));
                        }
                    }
                }
                Event::Readable(id) => {
                    if let Some(waker) = st.waits.entry(id.0).or_default().read.take() {
                        waker.wake();
                    }
                }
                Event::Writable(id) => {
                    if let Some(waker) = st.waits.entry(id.0).or_default().write.take() {
                        waker.wake();
                    }
                }
                Event::Closed(id, reason) => {
                    debug!("usbmux connection {} closed: {reason:?}", id.0);
                    let wait = st.waits.entry(id.0).or_default();
                    if let Some(tx) = wait.connect.take() {
                        let _ = tx.send(Err(ConnectError::DeviceGone));
                    }
                    for waker in [wait.read.take(), wait.write.take()].into_iter().flatten() {
                        waker.wake();
                    }
                }
                Event::DeviceMessage { level, text } => match level {
                    DeviceLevel::Error => warn!("usbmux device error: {text}"),
                    DeviceLevel::Warning => warn!("usbmux device warning: {text}"),
                    _ => debug!("usbmux device: {text}"),
                },
            }
        }
    }

    fn read_loop(self: Arc<Self>) {
        let mut buf = vec![0u8; USB_MRU];
        while !self.stop.load(Ordering::Relaxed) {
            let timeout = {
                let st = self.lock();
                st.session
                    .next_timeout(self.now_ms())
                    .map(Duration::from_millis)
                    .unwrap_or(IDLE_POLL)
                    .min(IDLE_POLL)
            };
            match self.pipe.read(&mut buf, timeout) {
                Ok(n) => self.on_input(&buf[..n]),
                Err(err)
                    if matches!(
                        err.kind(),
                        io::ErrorKind::TimedOut | io::ErrorKind::Interrupted
                    ) =>
                {
                    self.on_tick()
                }
                Err(err) => {
                    warn!("usbmux read failed: {err}");
                    self.terminate(true);
                    return;
                }
            }
        }
    }

    fn write_loop(self: Arc<Self>, mut rx: mpsc::UnboundedReceiver<Vec<u8>>) {
        // An empty frame is the signal to stop.
        while let Some(frame) = rx.blocking_recv() {
            if frame.is_empty() || self.stop.load(Ordering::Relaxed) {
                return;
            }
            if let Err(err) = self.pipe.write(&frame) {
                warn!("usbmux write failed: {err}");
                self.terminate(true);
                return;
            }
        }
    }

    fn on_input(&self, chunk: &[u8]) {
        let mut st = self.lock();
        match st.session.handle_input(chunk, self.now_ms()) {
            Ok(()) => {}
            Err(err @ MuxError::UnsupportedVersion { .. }) => {
                warn!("usbmux handshake failed: {err}");
                if let Some(tx) = st.ready.take() {
                    let _ = tx.send(Err(err));
                }
                drop(st);
                self.terminate(true);
                return;
            }
            Err(err) => warn!("dropped a usbmux packet: {err}"),
        }
        self.pump(&mut st);
    }

    fn on_tick(&self) {
        let mut st = self.lock();
        st.session.tick(self.now_ms());
        self.pump(&mut st);
    }

    fn terminate(&self, report: bool) {
        let callback = {
            let mut st = self.lock();
            if st.gone {
                return;
            }
            st.gone = true;
            st.session.device_removed();
            self.pump(&mut st);
            if let Some(tx) = st.ready.take() {
                let _ = tx.send(Err(MuxError::DeviceGone));
            }
            if report {
                self.on_gone
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .take()
            } else {
                None
            }
        };
        self.stop.store(true, Ordering::Relaxed);
        let _ = self.tx.send(Vec::new());
        if let Some(callback) = callback {
            callback();
        }
    }

    fn close_conn(&self, id: ConnId) {
        let mut st = self.lock();
        let _ = st.session.close(id);
        st.waits.remove(&id.0);
        self.pump(&mut st);
    }
}

/// One connection to a port of the device. Dropping it closes the
/// connection.
pub struct DeviceStream {
    link: Arc<Link>,
    id: ConnId,
}

impl std::fmt::Debug for DeviceStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeviceStream").finish_non_exhaustive()
    }
}

impl DeviceStream {
    pub fn close(&mut self) {
        self.link.close_conn(self.id);
    }
}

impl Drop for DeviceStream {
    fn drop(&mut self) {
        self.close();
    }
}

fn to_io_error(err: MuxError) -> io::Error {
    match err {
        MuxError::ConnectionClosed(CloseReason::InputOverflow) => {
            io::Error::new(io::ErrorKind::InvalidData, err)
        }
        MuxError::ConnectionClosed(_) | MuxError::DeviceGone | MuxError::UnknownConnection(_) => {
            io::Error::new(io::ErrorKind::BrokenPipe, err)
        }
        other => io::Error::other(other),
    }
}

impl AsyncRead for DeviceStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        let mut st = this.link.lock();
        let slice = buf.initialize_unfilled();
        if slice.is_empty() {
            return Poll::Ready(Ok(()));
        }
        match st.session.read(this.id, slice) {
            Ok(0) => {
                st.waits.entry(this.id.0).or_default().read = Some(cx.waker().clone());
                Poll::Pending
            }
            Ok(n) => {
                buf.advance(n);
                // Reading may have produced an acknowledgement.
                this.link.pump(&mut st);
                Poll::Ready(Ok(()))
            }
            // The device closes a connection by resetting it; that is an
            // ordinary end of stream once the buffered data is consumed.
            Err(MuxError::ConnectionClosed(CloseReason::PeerReset | CloseReason::PeerClosed)) => {
                Poll::Ready(Ok(()))
            }
            Err(err) => Poll::Ready(Err(to_io_error(err))),
        }
    }
}

impl AsyncWrite for DeviceStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        if buf.is_empty() {
            return Poll::Ready(Ok(0));
        }
        let mut st = this.link.lock();
        match st.session.write(this.id, buf) {
            Ok(0) => {
                st.waits.entry(this.id.0).or_default().write = Some(cx.waker().clone());
                Poll::Pending
            }
            Ok(n) => {
                this.link.pump(&mut st);
                Poll::Ready(Ok(n))
            }
            Err(err) => Poll::Ready(Err(to_io_error(err))),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        // Written data is already queued for the writer thread.
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        // The protocol has no half-close; the connection ends on drop.
        Poll::Ready(Ok(()))
    }
}
