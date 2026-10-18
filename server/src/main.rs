use anyhow::{anyhow, Context, Result};
use nusb::{MaybeFuture, transfer::{Bulk, In, Out, ControlIn, ControlOut, ControlType, Recipient}};
use portbridge_common::*;
use std::{env, io::{Read, Write}, path::{Path, PathBuf}, process::Command, sync::Arc};
use tokio::{net::{TcpListener, TcpStream}, sync::Semaphore, time::{timeout, Duration}};
use uuid::Uuid;

fn mock_device() -> DeviceDescriptor {
    DeviceDescriptor {
        id:"mock:portbridge-usb".into(),
        vid:Some(0x1209),
        pid:Some(0x5042),
        manufacturer:Some("PortBridge".into()),
        product:Some("Mock USB Device".into()),
        serial:Some("PB-MOCK-0001".into()),
        instance_id:Some("MOCK\\PORTBRIDGE".into()),
        mode:Some(UsbDeviceMode::Mock),
        interfaces:vec![UsbInterfaceDescriptor{number:0,class:0xff,subclass:0x42,protocol:0x01}],
    }
}

#[cfg(windows)]
fn list_usb_windows() -> Result<Vec<DeviceDescriptor>> {
    let script = r#"$ErrorActionPreference='Stop'; [Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false); $OutputEncoding = [Console]::OutputEncoding; Get-PnpDevice -PresentOnly | Where-Object { $_.InstanceId -like 'USB*' } | Select-Object FriendlyName,Manufacturer,InstanceId | ConvertTo-Json -Compress"#;
    let out = Command::new("powershell.exe")
        .args(["-NoProfile","-ExecutionPolicy","Bypass","-Command",script])
        .output()
        .context("start PowerShell USB enumeration")?;
    if !out.status.success() {
        return Err(anyhow!("PowerShell failed: {}", String::from_utf8_lossy(&out.stderr)));
    }
    let raw = String::from_utf8_lossy(&out.stdout);
    let val: serde_json::Value = serde_json::from_str(raw.trim()).unwrap_or(serde_json::Value::Array(vec![]));
    let items: Vec<serde_json::Value> = match val {
        serde_json::Value::Array(a)=>a,
        serde_json::Value::Null=>vec![],
        v=>vec![v]
    };
    Ok(items.into_iter().map(|v| {
        let instance = v.get("InstanceId").and_then(|x|x.as_str()).unwrap_or("").to_string();
        let up = instance.to_uppercase();
        fn hex4(s:&str,key:&str)->Option<u16>{
            s.find(key)
                .and_then(|i| s.get(i+key.len()..i+key.len()+4))
                .and_then(|x|u16::from_str_radix(x,16).ok())
        }
        DeviceDescriptor {
            id: instance.clone(),
            vid:hex4(&up,"VID_"),
            pid:hex4(&up,"PID_"),
            manufacturer:v.get("Manufacturer").and_then(|x|x.as_str()).map(str::to_string),
            product:v.get("FriendlyName").and_then(|x|x.as_str()).map(str::to_string),
            serial:None,
            instance_id:Some(instance),
            mode:Some(UsbDeviceMode::Unknown),
            interfaces:vec![]
        }
    }).collect())
}

#[cfg(not(windows))]
fn list_usb_windows() -> Result<Vec<DeviceDescriptor>> { Ok(vec![]) }

#[cfg(windows)]
fn adb_candidates() -> Vec<PathBuf> {
    let mut v = vec![PathBuf::from("adb.exe"), PathBuf::from("platform-tools").join("adb.exe")];
    for var in ["ANDROID_SDK_ROOT", "ANDROID_HOME"] {
        if let Ok(root) = env::var(var) {
            if !root.trim().is_empty() {
                v.push(Path::new(&root).join("platform-tools").join("adb.exe"));
            }
        }
    }
    if let Ok(local) = env::var("LOCALAPPDATA") {
        v.push(Path::new(&local).join("Android").join("Sdk").join("platform-tools").join("adb.exe"));
    }
    v
}

#[cfg(windows)]
fn find_adb() -> Option<PathBuf> {
    for p in adb_candidates() {
        if p.is_file() {
            return Some(p);
        }
    }
    if let Ok(out) = Command::new("where.exe").arg("adb.exe").output() {
        if out.status.success() {
            if let Some(line) = String::from_utf8_lossy(&out.stdout).lines().find(|x| !x.trim().is_empty()) {
                return Some(PathBuf::from(line.trim()));
            }
        }
    }
    None
}

#[cfg(not(windows))]
fn find_adb() -> Option<PathBuf> { None }

fn probe_adb() -> Result<()> {
    let Some(adb) = find_adb() else {
        println!("[ADB] adb.exe not found.");
        println!("[ADB] This does not mean the USB driver is broken; only that Android platform-tools are not installed or not discoverable.");
        return Ok(());
    };
    println!("[ADB] executable: {}", adb.display());
    let version = Command::new(&adb).arg("version").output().context("run adb version")?;
    print!("{}", String::from_utf8_lossy(&version.stdout));
    if !version.status.success() {
        return Err(anyhow!("adb version failed: {}", String::from_utf8_lossy(&version.stderr)));
    }

    let out = Command::new(&adb)
        .args(["devices", "-l"])
        .output()
        .context("run adb devices -l")?;
    println!("[ADB] devices:");
    print!("{}", String::from_utf8_lossy(&out.stdout));
    if !out.status.success() {
        return Err(anyhow!("adb devices failed: {}", String::from_utf8_lossy(&out.stderr)));
    }
    Ok(())
}

fn classify_usb_mode_enum(vid: u16, product: Option<&str>, interfaces: &[(u8,u8,u8,u8)]) -> UsbDeviceMode {
    let p = product.unwrap_or("").to_ascii_lowercase();

    if interfaces.iter().any(|(_, class, subclass, protocol)| *class == 0xff && *subclass == 0x42 && *protocol == 0x03)
        || p.contains("fastboot") {
        return UsbDeviceMode::Fastboot;
    }

    let adb_like = interfaces.iter().any(|(_, class, subclass, protocol)| *class == 0xff && *subclass == 0x42 && *protocol == 0x01);
    if vid == 0x2717 && adb_like {
        return UsbDeviceMode::XiaomiCandidate;
    }
    if adb_like || p.contains("adb") {
        return UsbDeviceMode::AdbLike;
    }
    if vid == 0x2717 {
        return UsbDeviceMode::XiaomiCandidate;
    }
    UsbDeviceMode::Other
}

fn mode_name(mode: &UsbDeviceMode) -> &'static str {
    match mode {
        UsbDeviceMode::Unknown => "UNKNOWN",
        UsbDeviceMode::AdbLike => "ADB_LIKE",
        UsbDeviceMode::Fastboot => "FASTBOOT",
        UsbDeviceMode::XiaomiCandidate => "XIAOMI_ADB_OR_MI_ASSISTANT_CANDIDATE",
        UsbDeviceMode::Other => "OTHER_USB",
        UsbDeviceMode::Mock => "MOCK",
    }
}

fn list_usb_low_level() -> Result<Vec<DeviceDescriptor>> {
    let devices = nusb::list_devices().wait().context("nusb list_devices")?;
    Ok(devices.map(|dev| {
        let raw_interfaces: Vec<(u8,u8,u8,u8)> = dev.interfaces()
            .map(|i| (i.interface_number(), i.class(), i.subclass(), i.protocol()))
            .collect();
        let interfaces = raw_interfaces.iter()
            .map(|(number,class,subclass,protocol)| UsbInterfaceDescriptor{
                number:*number, class:*class, subclass:*subclass, protocol:*protocol
            })
            .collect();
        let mode = classify_usb_mode_enum(dev.vendor_id(), dev.product_string(), &raw_interfaces);
        DeviceDescriptor {
            id: format!("nusb:{:?}", dev.id()),
            vid:Some(dev.vendor_id()),
            pid:Some(dev.product_id()),
            manufacturer:dev.manufacturer_string().map(str::to_string),
            product:dev.product_string().map(str::to_string),
            serial:dev.serial_number().map(str::to_string),
            instance_id:None,
            mode:Some(mode),
            interfaces,
        }
    }).collect())
}

fn probe_raw_usb() -> Result<()> {
    println!("[RAW-USB] Enumerating devices through nusb...");
    let devices = nusb::list_devices().wait().context("nusb list_devices")?;
    let mut count = 0usize;

    for dev in devices {
        count += 1;
        let interfaces: Vec<(u8,u8,u8,u8)> = dev.interfaces()
            .map(|i| (i.interface_number(), i.class(), i.subclass(), i.protocol()))
            .collect();

        let mode = classify_usb_mode_enum(dev.vendor_id(), dev.product_string(), &interfaces);
        println!(
            "[RAW-USB] {:04x}:{:04x} | mode={} | product={} | serial={}",
            dev.vendor_id(),
            dev.product_id(),
            mode_name(&mode),
            dev.product_string().unwrap_or(""),
            dev.serial_number().unwrap_or("")
        );

        if interfaces.is_empty() {
            println!("  interfaces: unavailable from OS cache");
        } else {
            for (num, class, subclass, protocol) in &interfaces {
                println!("  interface {} class={:02x} subclass={:02x} protocol={:02x}", num, class, subclass, protocol);
            }
        }

        match dev.open().wait() {
            Ok(_) => println!("  open-test: OK"),
            Err(e) => println!("  open-test: FAILED ({e})"),
        }
    }

    if count == 0 {
        println!("[RAW-USB] No USB devices returned by nusb.");
    }
    println!("[RAW-USB] Note: open-test does not send data and does not claim interfaces.");
    Ok(())
}



fn probe_claim_interfaces() -> Result<()> {
    println!("[CLAIM] Safe interface-claim test. No USB transfers will be sent.");
    let devices = nusb::list_devices().wait().context("nusb list_devices")?;
    let mut candidates = 0usize;

    for dev in devices {
        let raw_interfaces: Vec<(u8,u8,u8,u8)> = dev.interfaces()
            .map(|i| (i.interface_number(), i.class(), i.subclass(), i.protocol()))
            .collect();
        let mode = classify_usb_mode_enum(dev.vendor_id(), dev.product_string(), &raw_interfaces);

        for (num, class, subclass, protocol) in raw_interfaces.iter().copied() {
            let android_vendor_interface =
                class == 0xff && subclass == 0x42 && (protocol == 0x01 || protocol == 0x03);
            let xiaomi_vendor_interface = dev.vendor_id() == 0x2717 && class == 0xff;

            if !android_vendor_interface && !xiaomi_vendor_interface {
                continue;
            }

            candidates += 1;
            println!(
                "[CLAIM] {:04x}:{:04x} mode={} if={} class={:02x}/{:02x}/{:02x} product={}",
                dev.vendor_id(),
                dev.product_id(),
                mode_name(&mode),
                num,
                class,
                subclass,
                protocol,
                dev.product_string().unwrap_or("")
            );

            match dev.open().wait() {
                Ok(device) => match device.claim_interface(num).wait() {
                    Ok(interface) => {
                        println!("  claim-test: OK (interface released immediately)");
                        drop(interface);
                    }
                    Err(e) => println!("  claim-test: FAILED ({e})"),
                },
                Err(e) => println!("  device-open: FAILED ({e})"),
            }
        }
    }

    if candidates == 0 {
        println!("[CLAIM] No Android/Fastboot/Xiaomi vendor interfaces found.");
    }
    println!("[CLAIM] Finished. No control, bulk, interrupt, or isochronous transfers were submitted.");
    Ok(())
}

fn snapshot_usb_keys() -> Result<Vec<String>> {
    let mut keys: Vec<String> = list_usb_low_level()?
        .into_iter()
        .map(|d| format!("{:04x}:{:04x}|{:?}|{}|{}",
            d.vid.unwrap_or(0),
            d.pid.unwrap_or(0),
            d.mode.unwrap_or(UsbDeviceMode::Unknown),
            d.product.unwrap_or_default(),
            d.serial.unwrap_or_default()))
        .collect();
    keys.sort();
    Ok(keys)
}

async fn watch_usb_changes() -> Result<()> {
    println!("[WATCH] Monitoring USB re-enumeration. Press Ctrl+C to stop.");
    let mut last = snapshot_usb_keys().unwrap_or_default();
    for x in &last {
        println!("[WATCH] present: {x}");
    }

    loop {
        tokio::time::sleep(Duration::from_millis(500)).await;
        let now = match snapshot_usb_keys() {
            Ok(v) => v,
            Err(e) => {
                eprintln!("[WATCH] snapshot failed: {e:#}");
                continue;
            }
        };

        for old in last.iter().filter(|x| !now.contains(x)) {
            println!("[WATCH] removed: {old}");
        }
        for new in now.iter().filter(|x| !last.contains(x)) {
            println!("[WATCH] added:   {new}");
        }
        last = now;
    }
}


fn open_and_claim_by_id(device_id: &str) -> Result<nusb::Interface> {
    let devices = nusb::list_devices().wait().context("nusb list_devices")?;
    for dev in devices {
        let id = format!("nusb:{:?}", dev.id());
        if id != device_id {
            continue;
        }

        let raw_interfaces: Vec<(u8,u8,u8,u8)> = dev.interfaces()
            .map(|i| (i.interface_number(), i.class(), i.subclass(), i.protocol()))
            .collect();
        let mode = classify_usb_mode_enum(dev.vendor_id(), dev.product_string(), &raw_interfaces);

        let preferred = raw_interfaces.iter()
            .find(|(_,class,subclass,protocol)| {
                *class == 0xff && *subclass == 0x42 && (*protocol == 0x01 || *protocol == 0x03)
            })
            .or_else(|| {
                if dev.vendor_id() == 0x2717 {
                    raw_interfaces.iter().find(|(_,class,_,_)| *class == 0xff)
                } else {
                    None
                }
            })
            .or_else(|| raw_interfaces.first())
            .ok_or_else(|| anyhow!("device has no claimable USB interfaces"))?;

        let interface_number = preferred.0;
        let device = dev.open().wait().context("open USB device")?;
        let interface = device.claim_interface(interface_number).wait()
            .with_context(|| format!("claim interface {} for mode {}", interface_number, mode_name(&mode)))?;
        return Ok(interface);
    }
    Err(anyhow!("USB device not found: {device_id}"))
}



fn probe_bulk_endpoints(interface: &nusb::Interface) -> Vec<UsbEndpointProbe> {
    let mut out = Vec::new();
    for ep in 1u8..=15 {
        let in_addr = 0x80 | ep;
        match interface.endpoint::<Bulk, In>(in_addr) {
            Ok(_) => out.push(UsbEndpointProbe{
                address: in_addr,
                direction: UsbDirection::In,
                max_packet_size: 0,
                open_ok: true,
                error: None,
            }),
            Err(e) => {
                let msg=e.to_string();
                if !msg.to_ascii_lowercase().contains("not found") && !msg.to_ascii_lowercase().contains("invalid") {
                    out.push(UsbEndpointProbe{
                        address: in_addr,
                        direction: UsbDirection::In,
                        max_packet_size: 0,
                        open_ok: false,
                        error: Some(msg),
                    });
                }
            }
        }

        let out_addr = ep;
        match interface.endpoint::<Bulk, Out>(out_addr) {
            Ok(_) => out.push(UsbEndpointProbe{
                address: out_addr,
                direction: UsbDirection::Out,
                max_packet_size: 0,
                open_ok: true,
                error: None,
            }),
            Err(e) => {
                let msg=e.to_string();
                if !msg.to_ascii_lowercase().contains("not found") && !msg.to_ascii_lowercase().contains("invalid") {
                    out.push(UsbEndpointProbe{
                        address: out_addr,
                        direction: UsbDirection::Out,
                        max_packet_size: 0,
                        open_ok: false,
                        error: Some(msg),
                    });
                }
            }
        }
    }
    out
}

fn parse_control_setup(setup: [u8;8]) -> Result<(ControlType, Recipient, u8, u16, u16, u16, bool)> {
    let bm = setup[0];
    let control_type = match (bm >> 5) & 0x03 {
        0 => ControlType::Standard,
        1 => ControlType::Class,
        2 => ControlType::Vendor,
        _ => return Err(anyhow!("reserved USB control request type")),
    };
    let recipient = match bm & 0x1f {
        0 => Recipient::Device,
        1 => Recipient::Interface,
        2 => Recipient::Endpoint,
        3 => Recipient::Other,
        x => return Err(anyhow!("unsupported USB control recipient {x}")),
    };
    let request = setup[1];
    let value = u16::from_le_bytes([setup[2], setup[3]]);
    let index = u16::from_le_bytes([setup[4], setup[5]]);
    let length = u16::from_le_bytes([setup[6], setup[7]]);
    let is_in = (bm & 0x80) != 0;
    Ok((control_type, recipient, request, value, index, length, is_in))
}

fn perform_real_usb_request(interface: &nusb::Interface, request: &UsbRequest) -> Result<UsbCompletion> {
    validate_usb_request(request)?;

    let io_timeout = std::time::Duration::from_millis(request.timeout_ms as u64);
    match request.transfer {
        UsbTransferType::Control => {
            let setup = request.setup.ok_or_else(|| anyhow!("control transfer missing setup packet"))?;
            let (control_type, recipient, usb_request, value, index, setup_length, is_in) = parse_control_setup(setup)?;
            if is_in != matches!(request.direction, UsbDirection::In) {
                return Err(anyhow!("control direction mismatch"));
            }
            if is_in {
                let length = if request.expected_length == 0 { setup_length } else { request.expected_length.min(setup_length as u32) as u16 };
                let data = interface.control_in(ControlIn {
                    control_type,
                    recipient,
                    request: usb_request,
                    value,
                    index,
                    length,
                }, io_timeout).wait().context("USB control IN")?;
                Ok(UsbCompletion { request_id: request.request_id, status: UsbStatus::Ok, transferred: data.len() as u32, data })
            } else {
                if request.data.len() > setup_length as usize {
                    return Err(anyhow!("control OUT payload exceeds wLength"));
                }
                interface.control_out(ControlOut {
                    control_type,
                    recipient,
                    request: usb_request,
                    value,
                    index,
                    data: &request.data,
                }, io_timeout).wait().context("USB control OUT")?;
                Ok(UsbCompletion { request_id: request.request_id, status: UsbStatus::Ok, transferred: request.data.len() as u32, data: vec![] })
            }
        }
        UsbTransferType::Bulk => {
            if matches!(request.direction, UsbDirection::In) {
                let size = request.expected_length.max(1) as usize;
                let mut reader = interface.endpoint::<Bulk, In>(request.endpoint)
                    .context("open bulk IN endpoint")?
                    .reader(size.max(4096))
                    .with_read_timeout(io_timeout);
                let mut data = vec![0u8; size];
                let n = reader.read(&mut data).context("bulk IN read")?;
                data.truncate(n);
                Ok(UsbCompletion { request_id: request.request_id, status: UsbStatus::Ok, transferred: n as u32, data })
            } else {
                let mut writer = interface.endpoint::<Bulk, Out>(request.endpoint)
                    .context("open bulk OUT endpoint")?
                    .writer(request.data.len().max(4096));
                writer.write_all(&request.data).context("bulk OUT write")?;
                writer.flush().context("bulk OUT flush")?;
                Ok(UsbCompletion { request_id: request.request_id, status: UsbStatus::Ok, transferred: request.data.len() as u32, data: vec![] })
            }
        }
        UsbTransferType::Interrupt => Err(anyhow!("interrupt forwarding not implemented yet")),
        UsbTransferType::Isochronous => Err(anyhow!("isochronous forwarding not implemented")),
    }
}

async fn handle(mut s: TcpStream, mock: bool) -> Result<()> {
    s.set_nodelay(true)?;
    let hello: ControlMessage = timeout(Duration::from_secs(30), read_frame(&mut s)).await??;
    match hello {
        ControlMessage::Hello { version, .. } if version == PROTOCOL_VERSION => {},
        ControlMessage::Hello { version, .. } => {
            write_frame(&mut s, &ControlMessage::Error{message:format!("protocol mismatch server={} client={}",PROTOCOL_VERSION,version)}).await?;
            return Ok(());
        },
        _ => return Err(anyhow!("expected hello")),
    }
    let sid = Uuid::new_v4().to_string();
    write_frame(&mut s, &ControlMessage::Welcome{version:PROTOCOL_VERSION,session_id:sid}).await?;
    let mut opened: Option<String> = None;
    let mut real_interface: Option<nusb::Interface> = None;
    loop {
        let msg: ControlMessage = read_frame(&mut s).await?;
        match msg {
            ControlMessage::Ping => write_frame(&mut s,&ControlMessage::Pong).await?,
            ControlMessage::ListDevices => {
                let mut d = list_usb_low_level().or_else(|_| list_usb_windows()).unwrap_or_default();
                if mock { d.push(mock_device()); }
                write_frame(&mut s,&ControlMessage::DeviceList{devices:d}).await?;
            }
            ControlMessage::OpenUsb{device_id} => {
                if mock {
                    opened=Some(device_id.clone());
                    write_frame(&mut s,&ControlMessage::UsbOpened{device_id}).await?;
                } else {
                    match open_and_claim_by_id(&device_id) {
                        Ok(interface) => {
                            real_interface = Some(interface);
                            opened = Some(device_id.clone());
                            write_frame(&mut s,&ControlMessage::UsbOpened{device_id}).await?;
                        }
                        Err(e) => {
                            write_frame(&mut s,&ControlMessage::Error{message:format!("USB open/claim failed: {e:#}")}).await?;
                        }
                    }
                }
            }
            ControlMessage::UsbRequest{request} => {
                if !mock {
                    let Some(interface) = real_interface.as_ref() else {
                        write_frame(&mut s,&ControlMessage::Error{message:"no real USB interface opened".into()}).await?;
                        continue;
                    };
                    match perform_real_usb_request(interface, &request) {
                        Ok(completion) => write_frame(&mut s,&ControlMessage::UsbCompletion{completion}).await?,
                        Err(e) => {
                            eprintln!("USB request {} failed: {e:#}", request.request_id);
                            let completion=UsbCompletion{request_id:request.request_id,status:UsbStatus::Error,transferred:0,data:vec![]};
                            write_frame(&mut s,&ControlMessage::UsbCompletion{completion}).await?;
                        }
                    }
                    continue;
                }
                if opened.is_none() {
                    write_frame(&mut s,&ControlMessage::Error{message:"no USB device opened".into()}).await?;
                    continue;
                }
                let data = match request.direction {
                    UsbDirection::Out => vec![],
                    UsbDirection::In => {
                        if matches!(request.transfer, UsbTransferType::Control) {
                            vec![0x12,0x01,0x00,0x02,0,0,0,64]
                        } else {
                            b"PORTBRIDGE-MOCK".to_vec()
                        }
                    }
                };
                let c=UsbCompletion{request_id:request.request_id,status:UsbStatus::Ok,transferred:data.len() as u32,data};
                write_frame(&mut s,&ControlMessage::UsbCompletion{completion:c}).await?;
            }
            ControlMessage::ProbeBulkEndpoints => {
                if mock {
                    let endpoints=vec![
                        UsbEndpointProbe{address:0x81,direction:UsbDirection::In,max_packet_size:64,open_ok:true,error:None},
                        UsbEndpointProbe{address:0x01,direction:UsbDirection::Out,max_packet_size:64,open_ok:true,error:None},
                    ];
                    write_frame(&mut s,&ControlMessage::BulkEndpointProbeResult{endpoints}).await?;
                } else {
                    let Some(interface) = real_interface.as_ref() else {
                        write_frame(&mut s,&ControlMessage::Error{message:"no real USB interface opened".into()}).await?;
                        continue;
                    };
                    let endpoints=probe_bulk_endpoints(interface);
                    write_frame(&mut s,&ControlMessage::BulkEndpointProbeResult{endpoints}).await?;
                }
            }
            ControlMessage::ResetUsb{..} => {
                if mock {
                    write_frame(&mut s,&ControlMessage::Pong).await?
                } else {
                    write_frame(&mut s,&ControlMessage::Error{message:"real USB reset backend is not enabled yet".into()}).await?
                }
            }
            ControlMessage::CancelUsb{request_id} => {
                let c=UsbCompletion{request_id,status:UsbStatus::Cancelled,transferred:0,data:vec![]};
                write_frame(&mut s,&ControlMessage::UsbCompletion{completion:c}).await?;
            }
            ControlMessage::Close => break,
            _ => write_frame(&mut s,&ControlMessage::Error{message:"unsupported message".into()}).await?,
        }
    }
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let args:Vec<String>=env::args().collect();

    if args.iter().any(|a|a=="--list-usb") {
        let mut d=list_usb_windows()?;
        if args.iter().any(|a|a=="--mock-usb") { d.push(mock_device()); }
        if d.is_empty(){
            println!("No present USB PnP devices found.");
        } else {
            for x in d {
                println!("{} {:04x}:{:04x} | {} | {}", x.id, x.vid.unwrap_or(0), x.pid.unwrap_or(0), x.manufacturer.unwrap_or_default(), x.product.unwrap_or_default());
            }
        }
        return Ok(());
    }

    if args.iter().any(|a|a=="--probe-adb") {
        probe_adb()?;
        return Ok(());
    }

    if args.iter().any(|a|a=="--probe-raw-usb") {
        probe_raw_usb()?;
        return Ok(());
    }

    if args.iter().any(|a|a=="--watch-usb") {
        watch_usb_changes().await?;
        return Ok(());
    }

    if args.iter().any(|a|a=="--probe-claim") {
        probe_claim_interfaces()?;
        return Ok(());
    }

    let mock=args.iter().any(|a|a=="--mock-usb");
    let addr=args.iter().find(|a|a.contains(':') && !a.starts_with("--")).cloned().unwrap_or_else(||"0.0.0.0:47990".into());
    let listener=TcpListener::bind(&addr).await?;
    println!("PortBridge server v0.19 listening on {addr} mock_usb={mock}");
    let sem=Arc::new(Semaphore::new(128));
    loop {
        let (s,p)=listener.accept().await?;
        let permit=sem.clone().acquire_owned().await?;
        tokio::spawn(async move {
            let _permit=permit;
            if let Err(e)=handle(s,mock).await { eprintln!("{p}: {e:#}"); }
        });
    }
}
