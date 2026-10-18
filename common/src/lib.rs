use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub const PROTOCOL_VERSION: u16 = 12;
pub const MAX_FRAME: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UsbDeviceMode {
    Unknown,
    AdbLike,
    Fastboot,
    XiaomiCandidate,
    Other,
    Mock,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsbInterfaceDescriptor {
    pub number: u8,
    pub class: u8,
    pub subclass: u8,
    pub protocol: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsbEndpointProbe {
    pub address: u8,
    pub direction: UsbDirection,
    pub max_packet_size: u32,
    pub open_ok: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceDescriptor {
    pub id: String,
    pub vid: Option<u16>,
    pub pid: Option<u16>,
    pub manufacturer: Option<String>,
    pub product: Option<String>,
    pub serial: Option<String>,
    pub instance_id: Option<String>,
    #[serde(default)]
    pub mode: Option<UsbDeviceMode>,
    #[serde(default)]
    pub interfaces: Vec<UsbInterfaceDescriptor>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ControlMessage {
    Hello { version: u16, client: String },
    Welcome { version: u16, session_id: String },
    Ping,
    Pong,
    ListDevices,
    DeviceList { devices: Vec<DeviceDescriptor> },
    OpenUsb { device_id: String },
    UsbOpened { device_id: String },
    UsbRequest { request: UsbRequest },
    UsbCompletion { completion: UsbCompletion },
    ProbeBulkEndpoints,
    BulkEndpointProbeResult { endpoints: Vec<UsbEndpointProbe> },
    ResetUsb { device_id: String },
    CancelUsb { request_id: u64 },
    Close,
    Error { message: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsbRequest {
    pub request_id: u64,
    pub endpoint: u8,
    pub transfer: UsbTransferType,
    pub direction: UsbDirection,
    pub timeout_ms: u32,
    pub setup: Option<[u8; 8]>,
    pub data: Vec<u8>,
    pub expected_length: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UsbTransferType { Control, Bulk, Interrupt, Isochronous }

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UsbDirection { In, Out }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsbCompletion {
    pub request_id: u64,
    pub status: UsbStatus,
    pub data: Vec<u8>,
    pub transferred: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UsbStatus { Ok, Cancelled, Timeout, Stall, Disconnected, Error }

pub async fn write_frame<W: AsyncWrite + Unpin, T: Serialize>(w: &mut W, value: &T) -> Result<()> {
    let data = serde_json::to_vec(value)?;
    if data.len() > MAX_FRAME { return Err(anyhow!("frame too large")); }
    w.write_u32(data.len() as u32).await?;
    w.write_all(&data).await?;
    w.flush().await?;
    Ok(())
}

pub async fn read_frame<R: AsyncRead + Unpin, T: for<'de> Deserialize<'de>>(r: &mut R) -> Result<T> {
    let len = r.read_u32().await? as usize;
    if len > MAX_FRAME { return Err(anyhow!("frame too large: {len}")); }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf).await?;
    Ok(serde_json::from_slice(&buf)?)
}


pub fn validate_usb_request(request: &UsbRequest) -> Result<()> {
    if request.timeout_ms == 0 || request.timeout_ms > 120_000 {
        return Err(anyhow!("invalid USB timeout: {}", request.timeout_ms));
    }
    if request.data.len() > MAX_FRAME {
        return Err(anyhow!("USB payload too large: {}", request.data.len()));
    }
    if request.expected_length as usize > MAX_FRAME {
        return Err(anyhow!("USB expected_length too large: {}", request.expected_length));
    }
    if matches!(request.transfer, UsbTransferType::Control) {
        let setup = request.setup.ok_or_else(|| anyhow!("control transfer requires setup packet"))?;
        let setup_len = u16::from_le_bytes([setup[6], setup[7]]) as usize;
        match request.direction {
            UsbDirection::In => {
                if request.expected_length as usize > setup_len && request.expected_length != 0 {
                    return Err(anyhow!("control IN expected_length exceeds wLength"));
                }
            }
            UsbDirection::Out => {
                if request.data.len() > setup_len {
                    return Err(anyhow!("control OUT payload exceeds wLength"));
                }
            }
        }
    }
    Ok(())
}
