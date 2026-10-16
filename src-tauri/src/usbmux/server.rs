//! The client facing half of the muxer: speaks the usbmuxd plist protocol so
//! that `idevice` (and anything else that talks to `usbmuxd`) works against
//! it unchanged.

use std::io;
use std::time::Duration;

use idevice::usbmuxd::RawPacket;
use idevice::usbmuxd::server::{
    UsbmuxdServerRequest as Request, UsbmuxdServerResponse as Response,
};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpListener;
use tracing::{debug, warn};

use super::hub::{AttachedDevice, Hub, HubConnectError, HubEvent};
use super::link::{ConnectError, DeviceStream};

const RESULT_OK: u32 = 0;
const RESULT_BAD_COMMAND: u32 = 1;
const RESULT_BAD_DEVICE: u32 = 2;
const RESULT_CONNECTION_REFUSED: u32 = 3;

/// How long a `ReadPairRecord` waits for a pairing that is under way.
const PAIR_RECORD_WAIT: Duration = Duration::from_secs(60);

/// Pair records are a few kilobytes; anything much bigger is not a request.
const MAX_REQUEST: usize = 1 << 20;

/// Serves clients on a unix socket until the listener fails.
#[cfg(unix)]
pub async fn serve_unix(listener: tokio::net::UnixListener, hub: Hub) {
    loop {
        match listener.accept().await {
            Ok((stream, _)) => spawn_client(stream, hub.clone()),
            Err(err) => {
                warn!("usbmux server stopped: {err}");
                return;
            }
        }
    }
}

/// Serves clients on a TCP listener until it fails.
pub async fn serve_tcp(listener: TcpListener, hub: Hub) {
    loop {
        match listener.accept().await {
            Ok((stream, _)) => spawn_client(stream, hub.clone()),
            Err(err) => {
                warn!("usbmux server stopped: {err}");
                return;
            }
        }
    }
}

fn spawn_client<S>(stream: S, hub: Hub)
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        if let Err(err) = handle_client(stream, hub).await {
            debug!("usbmux client ended: {err}");
        }
    });
}

async fn read_packet<S: AsyncRead + Unpin>(stream: &mut S) -> io::Result<Option<RawPacket>> {
    let mut header = [0u8; 16];
    match stream.read_exact(&mut header).await {
        Ok(_) => {}
        Err(err) if err.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(err) => return Err(err),
    }
    let size = u32::from_le_bytes([header[0], header[1], header[2], header[3]]) as usize;
    if !(16..=MAX_REQUEST).contains(&size) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "bad usbmux packet size",
        ));
    }
    let mut packet = header.to_vec();
    packet.resize(size, 0);
    stream.read_exact(&mut packet[16..]).await?;
    RawPacket::try_from(packet.as_slice())
        .map(Some)
        .map_err(|()| io::Error::new(io::ErrorKind::InvalidData, "unparsable usbmux packet"))
}

async fn send<S: AsyncWrite + Unpin>(
    stream: &mut S,
    tag: u32,
    response: Response,
) -> io::Result<()> {
    let bytes: Vec<u8> = response.into_packet(tag).into();
    stream.write_all(&bytes).await
}

fn device_dict(device: &AttachedDevice) -> plist::Dictionary {
    let mut properties = plist::Dictionary::new();
    properties.insert("ConnectionType".into(), "USB".into());
    properties.insert("ConnectionSpeed".into(), device.info.speed.into());
    properties.insert("DeviceID".into(), device.id.into());
    properties.insert("LocationID".into(), device.info.location_id.into());
    properties.insert("ProductID".into(), device.info.product_id.into());
    properties.insert("SerialNumber".into(), device.info.udid.clone().into());

    let mut dict = plist::Dictionary::new();
    dict.insert("DeviceID".into(), device.id.into());
    dict.insert("MessageType".into(), "Attached".into());
    dict.insert("Properties".into(), plist::Value::Dictionary(properties));
    dict
}

async fn handle_client<S>(mut stream: S, hub: Hub) -> io::Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    while let Some(packet) = read_packet(&mut stream).await? {
        let tag = packet.tag;

        // Not modelled by idevice's server types.
        if let Some(plist::Value::String(kind)) = packet.plist.get("MessageType") {
            if kind == "DeletePairRecord" {
                let result = match packet.plist.get("PairRecordID") {
                    Some(plist::Value::String(id)) if hub.pairs().delete(id).is_ok() => RESULT_OK,
                    _ => RESULT_BAD_COMMAND,
                };
                send(&mut stream, tag, Response::Result(result)).await?;
                continue;
            }
        }

        let request = match Request::decode(&packet) {
            Ok(request) => request,
            Err(err) => {
                debug!("rejecting usbmux request: {err}");
                send(&mut stream, tag, Response::Result(RESULT_BAD_COMMAND)).await?;
                continue;
            }
        };

        match request {
            Request::ListDevices => {
                let devices = hub
                    .devices()
                    .iter()
                    .map(|device| plist::Value::Dictionary(device_dict(device)))
                    .collect();
                send(&mut stream, tag, Response::DeviceList(devices)).await?;
            }
            Request::ReadBuid => {
                send(&mut stream, tag, Response::Buid(hub.pairs().buid())).await?;
            }
            Request::ReadPairRecord { pair_record_id } => {
                // Right after plugging in, the pair record only appears once
                // the user tapped "Trust" on the device.
                let record = hub
                    .wait_for_pair_record(&pair_record_id, PAIR_RECORD_WAIT)
                    .await;
                let response = match record {
                    Some(record) => Response::PairRecord(record),
                    None => Response::Result(RESULT_BAD_DEVICE),
                };
                send(&mut stream, tag, response).await?;
            }
            Request::SavePairRecord {
                pair_record_id,
                device_id,
                pair_record_data,
            } => {
                let udid = pair_record_id.or_else(|| {
                    let id = u32::try_from(device_id?).ok()?;
                    hub.devices()
                        .into_iter()
                        .find(|device| device.id == id)
                        .map(|device| device.info.udid)
                });
                let result = match udid {
                    Some(udid) if hub.pairs().save(&udid, &pair_record_data).is_ok() => RESULT_OK,
                    _ => RESULT_BAD_COMMAND,
                };
                send(&mut stream, tag, Response::Result(result)).await?;
            }
            Request::Listen => {
                send(&mut stream, tag, Response::Result(RESULT_OK)).await?;
                return listen(stream, hub, tag).await;
            }
            Request::Connect { device_id, port } => {
                let connected = match u32::try_from(device_id) {
                    Ok(id) => hub.connect(id, port).await,
                    Err(_) => Err(HubConnectError::NoSuchDevice),
                };
                match connected {
                    Ok(device) => {
                        send(&mut stream, tag, Response::Result(RESULT_OK)).await?;
                        return bridge(stream, device).await;
                    }
                    Err(HubConnectError::Connect(
                        ConnectError::Refused | ConnectError::Timeout,
                    )) => {
                        send(
                            &mut stream,
                            tag,
                            Response::Result(RESULT_CONNECTION_REFUSED),
                        )
                        .await?;
                    }
                    Err(_) => {
                        send(&mut stream, tag, Response::Result(RESULT_BAD_DEVICE)).await?;
                    }
                }
            }
        }
    }
    Ok(())
}

/// After `Listen` the client gets an `Attached` for every device that is
/// there already and from then on every change, until it hangs up.
async fn listen<S>(mut stream: S, hub: Hub, tag: u32) -> io::Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    // Subscribe first so that nothing slips in between listing and waiting.
    let mut events = hub.subscribe();
    for device in hub.devices() {
        send(&mut stream, tag, Response::Attached(device_dict(&device))).await?;
    }
    let mut scratch = [0u8; 256];
    loop {
        tokio::select! {
            event = events.recv() => match event {
                Ok(HubEvent::Attached(device)) => {
                    send(&mut stream, tag, Response::Attached(device_dict(&device))).await?;
                }
                Ok(HubEvent::Detached(id)) => {
                    send(&mut stream, tag, Response::Detached(u64::from(id))).await?;
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return Ok(()),
            },
            read = stream.read(&mut scratch) => match read {
                Ok(0) | Err(_) => return Ok(()),
                // Listeners have nothing more to say.
                Ok(_) => {}
            },
        }
    }
}

/// Once connected, the socket is a raw byte pipe to the device. Like
/// `usbmuxd`, either side hanging up ends both.
async fn bridge<S>(client: S, device: DeviceStream) -> io::Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let (mut client_read, mut client_write) = tokio::io::split(client);
    let (mut device_read, mut device_write) = tokio::io::split(device);
    tokio::select! {
        _ = tokio::io::copy(&mut client_read, &mut device_write) => {}
        _ = tokio::io::copy(&mut device_read, &mut client_write) => {}
    }
    let _ = client_write.flush().await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;
    use std::sync::Arc;
    use std::time::Duration;

    use futures::StreamExt;
    use idevice::IdeviceError;
    use idevice::usbmuxd::{
        Connection, UsbmuxdAddr, UsbmuxdConnection, UsbmuxdListenEvent, errors::UsbmuxdError,
    };

    use super::*;
    use crate::usbmux::hub::DeviceInfo;
    use crate::usbmux::pair_store::MemoryPairStore;
    use crate::usbmux::testutil::FakeDevice;

    const UDID: &str = "00008030-001C1D2E0A68802E";

    fn info(udid: &str) -> DeviceInfo {
        DeviceInfo {
            udid: udid.into(),
            product_id: 0x12a8,
            location_id: 0x0100_0000,
            speed: 480_000_000,
        }
    }

    async fn start() -> (Hub, UsbmuxdAddr) {
        let hub = Hub::new(Box::new(MemoryPairStore::new()));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr: SocketAddr = listener.local_addr().unwrap();
        tokio::spawn(serve_tcp(listener, hub.clone()));
        (hub, UsbmuxdAddr::TcpSocket(addr))
    }

    async fn attach(hub: &Hub, udid: &str) -> (u32, Arc<FakeDevice>) {
        let device = Arc::new(FakeDevice::with_services());
        let id = hub.add_device(device.clone(), info(udid)).await.unwrap();
        (id, device)
    }

    /// A hand rolled request, for messages `idevice`'s client has no method
    /// for or to inspect raw replies.
    async fn raw_request(addr: &UsbmuxdAddr, request: plist::Dictionary) -> plist::Dictionary {
        let mut socket = addr.to_socket().await.unwrap();
        let packet = RawPacket::new(
            request,
            UsbmuxdConnection::XML_PLIST_VERSION,
            UsbmuxdConnection::PLIST_MESSAGE_TYPE,
            7,
        );
        let bytes: Vec<u8> = packet.into();
        socket.write_all(&bytes).await.unwrap();
        let mut header = [0u8; 16];
        socket.read_exact(&mut header).await.unwrap();
        let size = u32::from_le_bytes(header[..4].try_into().unwrap()) as usize;
        assert_eq!(u32::from_le_bytes(header[12..].try_into().unwrap()), 7);
        let mut body = vec![0u8; size - 16];
        socket.read_exact(&mut body).await.unwrap();
        plist::from_bytes(&body).unwrap()
    }

    async fn next<S>(events: &mut S) -> UsbmuxdListenEvent
    where
        S: futures::Stream<Item = Result<UsbmuxdListenEvent, IdeviceError>> + Unpin,
    {
        tokio::time::timeout(Duration::from_secs(5), events.next())
            .await
            .expect("no event")
            .expect("stream ended")
            .expect("protocol error")
    }

    fn message(kind: &str) -> plist::Dictionary {
        let mut dict = plist::Dictionary::new();
        dict.insert("MessageType".into(), kind.into());
        dict
    }

    fn number(reply: &plist::Dictionary) -> u64 {
        reply
            .get("Number")
            .and_then(|value| value.as_unsigned_integer())
            .expect("reply has a Number")
    }

    #[tokio::test]
    async fn idevice_client_lists_the_device() {
        let (hub, addr) = start().await;
        let (id, _device) = attach(&hub, UDID).await;
        let mut client = addr.connect(1).await.unwrap();
        let devices = client.get_devices().await.unwrap();
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].udid, UDID);
        assert_eq!(devices[0].device_id, id);
        assert_eq!(devices[0].connection_type, Connection::Usb);
        assert_eq!(client.get_device(UDID).await.unwrap().device_id, id);
    }

    #[tokio::test]
    async fn empty_hub_lists_nothing() {
        let (_hub, addr) = start().await;
        let mut client = addr.connect(1).await.unwrap();
        assert!(client.get_devices().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn idevice_client_reaches_a_device_service() {
        let (hub, addr) = start().await;
        let (id, _device) = attach(&hub, UDID).await;
        let client = addr.connect(1).await.unwrap();
        let mut conn = client.connect_to_device(id, 7, "test").await.unwrap();
        conn.send_raw(b"hello over usb").await.unwrap();
        assert_eq!(conn.read_raw(14).await.unwrap(), b"hello over usb");
    }

    #[tokio::test]
    async fn connect_errors_use_the_usbmuxd_codes() {
        let (hub, addr) = start().await;
        let (id, _device) = attach(&hub, UDID).await;

        let refused = addr
            .connect(1)
            .await
            .unwrap()
            .connect_to_device(id, 4242, "test")
            .await
            .err()
            .unwrap();
        assert!(matches!(
            refused,
            IdeviceError::Usbmuxd(UsbmuxdError::ConnectionRefused)
        ));

        let no_device = addr
            .connect(1)
            .await
            .unwrap()
            .connect_to_device(id + 100, 7, "test")
            .await
            .err()
            .unwrap();
        assert!(matches!(
            no_device,
            IdeviceError::Usbmuxd(UsbmuxdError::BadDevice)
        ));
    }

    #[tokio::test]
    async fn buid_is_stable() {
        let (_hub, addr) = start().await;
        let first = addr.connect(1).await.unwrap().get_buid().await.unwrap();
        let second = addr.connect(1).await.unwrap().get_buid().await.unwrap();
        assert_eq!(first, "TEST-BUID");
        assert_eq!(first, second);
    }

    #[tokio::test]
    async fn pair_records_can_be_saved_read_and_deleted() {
        let (_hub, addr) = start().await;
        let mut save = message("SavePairRecord");
        save.insert("PairRecordID".into(), UDID.into());
        save.insert(
            "PairRecordData".into(),
            plist::Value::Data(b"record".to_vec()),
        );
        assert_eq!(number(&raw_request(&addr, save).await), 0);

        let mut read = message("ReadPairRecord");
        read.insert("PairRecordID".into(), UDID.into());
        let reply = raw_request(&addr, read.clone()).await;
        assert_eq!(
            reply.get("PairRecordData"),
            Some(&plist::Value::Data(b"record".to_vec()))
        );

        let mut delete = message("DeletePairRecord");
        delete.insert("PairRecordID".into(), UDID.into());
        assert_eq!(number(&raw_request(&addr, delete).await), 0);
        assert_eq!(number(&raw_request(&addr, read).await), 2);
    }

    #[tokio::test]
    async fn reading_a_record_waits_while_the_device_is_being_paired() {
        let (hub, addr) = start().await;
        hub.set_pair_state(UDID, crate::usbmux::pairing::PairState::WaitingForTrust);
        let late = hub.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            late.pairs().save(UDID, b"fresh").unwrap();
            late.set_pair_state(UDID, crate::usbmux::pairing::PairState::Paired);
        });
        let mut read = message("ReadPairRecord");
        read.insert("PairRecordID".into(), UDID.into());
        let reply = raw_request(&addr, read).await;
        assert_eq!(
            reply.get("PairRecordData"),
            Some(&plist::Value::Data(b"fresh".to_vec()))
        );
    }

    #[tokio::test]
    async fn pair_record_without_an_id_is_saved_under_the_device_udid() {
        let (hub, addr) = start().await;
        let (id, _device) = attach(&hub, UDID).await;
        let mut save = message("SavePairRecord");
        save.insert("DeviceID".into(), id.into());
        save.insert(
            "PairRecordData".into(),
            plist::Value::Data(b"by id".to_vec()),
        );
        assert_eq!(number(&raw_request(&addr, save).await), 0);
        assert_eq!(hub.pairs().read(UDID).as_deref(), Some(&b"by id"[..]));

        let mut nobody = message("SavePairRecord");
        nobody.insert("DeviceID".into(), 999u32.into());
        nobody.insert("PairRecordData".into(), plist::Value::Data(b"x".to_vec()));
        assert_eq!(number(&raw_request(&addr, nobody).await), 1);
    }

    #[tokio::test]
    async fn unknown_requests_are_answered_with_bad_command() {
        let (_hub, addr) = start().await;
        assert_eq!(number(&raw_request(&addr, message("Frobnicate")).await), 1);
        assert_eq!(
            number(&raw_request(&addr, plist::Dictionary::new()).await),
            1
        );
    }

    #[tokio::test]
    async fn listeners_hear_about_attach_and_detach() {
        let (hub, addr) = start().await;
        let (first, _device) = attach(&hub, UDID).await;

        let mut client = addr.connect(1).await.unwrap();
        let mut events = client.listen().await.unwrap();

        // Devices that were there before listening are replayed.
        match next(&mut events).await {
            UsbmuxdListenEvent::Connected(device) => {
                assert_eq!((device.device_id, device.udid.as_str()), (first, UDID));
            }
            other => panic!("unexpected {other:?}"),
        }

        let (second, device) = attach(&hub, "00008030-AAAAAAAAAAAAAAAA").await;
        match next(&mut events).await {
            UsbmuxdListenEvent::Connected(device) => assert_eq!(device.device_id, second),
            other => panic!("unexpected {other:?}"),
        }

        device.unplug();
        match next(&mut events).await {
            UsbmuxdListenEvent::Disconnected(id) => assert_eq!(id, second),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[tokio::test]
    async fn closing_the_client_closes_the_device_connection() {
        let (hub, addr) = start().await;
        let (id, device) = attach(&hub, UDID).await;
        let client = addr.connect(1).await.unwrap();
        let mut conn = client.connect_to_device(id, 7, "test").await.unwrap();
        conn.send_raw(b"x").await.unwrap();
        conn.read_raw(1).await.unwrap();
        assert_eq!(device.open_connections(), 1);
        drop(conn);
        tokio::time::timeout(Duration::from_secs(5), async {
            while device.open_connections() != 0 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("device connection stayed open");
    }

    #[tokio::test]
    async fn device_hanging_up_closes_the_client_socket() {
        let (hub, addr) = start().await;
        let (id, _device) = attach(&hub, UDID).await;
        let client = addr.connect(1).await.unwrap();
        let mut conn = client.connect_to_device(id, 8, "test").await.unwrap();
        conn.send_raw(b"bye").await.unwrap();
        assert_eq!(conn.read_raw(3).await.unwrap(), b"bye");
        // The device now resets the connection, which ends the stream.
        assert!(conn.read_raw(1).await.is_err());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn works_over_a_unix_socket_too() {
        let path = std::env::temp_dir().join(format!("ilmux-{}.sock", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let hub = Hub::new(Box::new(MemoryPairStore::new()));
        let listener = tokio::net::UnixListener::bind(&path).unwrap();
        tokio::spawn(serve_unix(listener, hub.clone()));
        let (id, _device) = attach(&hub, UDID).await;

        let addr = UsbmuxdAddr::UnixSocket(path.to_string_lossy().into_owned());
        let mut client = addr.connect(1).await.unwrap();
        assert_eq!(client.get_devices().await.unwrap()[0].device_id, id);
        let _ = std::fs::remove_file(path);
    }
}
