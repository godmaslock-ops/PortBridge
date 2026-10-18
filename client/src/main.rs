use anyhow::{anyhow, Result};
use portbridge_common::*;
use std::env;
use tokio::net::TcpStream;

async fn recv(s:&mut TcpStream)->Result<ControlMessage>{ read_frame(s).await }

async fn selftest(addr:&str)->Result<()> {
    let mut s=TcpStream::connect(addr).await?; s.set_nodelay(true)?;
    write_frame(&mut s,&ControlMessage::Hello{version:PROTOCOL_VERSION,client:"portbridge-selftest".into()}).await?;
    let sid=match recv(&mut s).await? { ControlMessage::Welcome{session_id,..}=>session_id, x=>return Err(anyhow!("bad welcome: {x:?}")) };
    write_frame(&mut s,&ControlMessage::ListDevices).await?;
    let devices=match recv(&mut s).await? { ControlMessage::DeviceList{devices}=>devices, x=>return Err(anyhow!("bad device list: {x:?}")) };
    let d=devices.into_iter().find(|d|d.id.starts_with("mock:")).ok_or_else(||anyhow!("mock device not found; start server with --mock-usb"))?;
    write_frame(&mut s,&ControlMessage::OpenUsb{device_id:d.id.clone()}).await?; match recv(&mut s).await?{ControlMessage::UsbOpened{..}=>{},x=>return Err(anyhow!("open failed: {x:?}"))}
    let req=UsbRequest{request_id:1,endpoint:0x81,transfer:UsbTransferType::Bulk,direction:UsbDirection::In,timeout_ms:2000,setup:None,data:vec![],expected_length:64};
    write_frame(&mut s,&ControlMessage::UsbRequest{request:req}).await?;
    match recv(&mut s).await? { ControlMessage::UsbCompletion{completion} if matches!(completion.status,UsbStatus::Ok) => {}, x=>return Err(anyhow!("bulk in failed: {x:?}")) }
    let ctl=UsbRequest{request_id:2,endpoint:0,transfer:UsbTransferType::Control,direction:UsbDirection::In,timeout_ms:2000,setup:Some([0x80,6,0,1,0,0,18,0]),data:vec![],expected_length:18};
    write_frame(&mut s,&ControlMessage::UsbRequest{request:ctl}).await?; match recv(&mut s).await?{ControlMessage::UsbCompletion{completion} if matches!(completion.status,UsbStatus::Ok)=>{},x=>return Err(anyhow!("control failed: {x:?}"))}
    write_frame(&mut s,&ControlMessage::CancelUsb{request_id:999}).await?; match recv(&mut s).await?{ControlMessage::UsbCompletion{completion} if matches!(completion.status,UsbStatus::Cancelled)=>{},x=>return Err(anyhow!("cancel failed: {x:?}"))}
    println!("USB self-test passed: session={sid} device={} {:04x}:{:04x}",d.id,d.vid.unwrap_or(0),d.pid.unwrap_or(0));
    Ok(())
}


async fn list_remote_usb(addr:&str)->Result<()> {
    let mut s=TcpStream::connect(addr).await?;
    s.set_nodelay(true)?;
    write_frame(&mut s,&ControlMessage::Hello{version:PROTOCOL_VERSION,client:"portbridge-list".into()}).await?;
    match recv(&mut s).await? {
        ControlMessage::Welcome{..} => {},
        x => return Err(anyhow!("bad welcome: {x:?}")),
    }
    write_frame(&mut s,&ControlMessage::ListDevices).await?;
    let devices=match recv(&mut s).await? {
        ControlMessage::DeviceList{devices} => devices,
        x => return Err(anyhow!("bad device list: {x:?}")),
    };
    for d in devices {
        println!(
            "{} {:04x}:{:04x} mode={:?} product={} serial={}",
            d.id,
            d.vid.unwrap_or(0),
            d.pid.unwrap_or(0),
            d.mode,
            d.product.unwrap_or_default(),
            d.serial.unwrap_or_default()
        );
        for i in d.interfaces {
            println!("  if={} class={:02x} subclass={:02x} protocol={:02x}", i.number, i.class, i.subclass, i.protocol);
        }
    }
    write_frame(&mut s,&ControlMessage::Close).await?;
    Ok(())
}



async fn remote_control_smoke(addr:&str)->Result<()> {
    let mut s=TcpStream::connect(addr).await?;
    s.set_nodelay(true)?;
    write_frame(&mut s,&ControlMessage::Hello{version:PROTOCOL_VERSION,client:"portbridge-remote-smoke".into()}).await?;
    match recv(&mut s).await? {
        ControlMessage::Welcome{..} => {},
        x => return Err(anyhow!("bad welcome: {x:?}")),
    }

    write_frame(&mut s,&ControlMessage::ListDevices).await?;
    let devices=match recv(&mut s).await? {
        ControlMessage::DeviceList{devices} => devices,
        x => return Err(anyhow!("bad device list: {x:?}")),
    };

    let d=devices.into_iter()
        .find(|d| matches!(d.mode, Some(UsbDeviceMode::AdbLike) | Some(UsbDeviceMode::XiaomiCandidate) | Some(UsbDeviceMode::Fastboot)))
        .ok_or_else(|| anyhow!("no Android/Xiaomi/Fastboot USB device found on remote PC"))?;

    println!(
        "[SMOKE] selected {} {:04x}:{:04x} mode={:?} product={}",
        d.id,
        d.vid.unwrap_or(0),
        d.pid.unwrap_or(0),
        d.mode,
        d.product.clone().unwrap_or_default()
    );

    write_frame(&mut s,&ControlMessage::OpenUsb{device_id:d.id.clone()}).await?;
    match recv(&mut s).await? {
        ControlMessage::UsbOpened{..} => println!("[SMOKE] remote open/claim: OK"),
        ControlMessage::Error{message} => return Err(anyhow!("remote open/claim failed: {message}")),
        x => return Err(anyhow!("unexpected open response: {x:?}")),
    }

    let req=UsbRequest{
        request_id:1001,
        endpoint:0,
        transfer:UsbTransferType::Control,
        direction:UsbDirection::In,
        timeout_ms:3000,
        setup:Some([0x80,0x06,0x00,0x01,0x00,0x00,18,0]),
        data:vec![],
        expected_length:18,
    };
    validate_usb_request(&req)?;
    write_frame(&mut s,&ControlMessage::UsbRequest{request:req}).await?;
    match recv(&mut s).await? {
        ControlMessage::UsbCompletion{completion} if matches!(completion.status,UsbStatus::Ok) => {
            println!("[SMOKE] control GET_DESCRIPTOR: OK transferred={}", completion.transferred);
            if completion.data.len() >= 12 {
                let vid=u16::from_le_bytes([completion.data[8],completion.data[9]]);
                let pid=u16::from_le_bytes([completion.data[10],completion.data[11]]);
                println!("[SMOKE] descriptor VID:PID={:04x}:{:04x}",vid,pid);
            }
        }
        ControlMessage::UsbCompletion{completion} => return Err(anyhow!("control request failed with status {:?}",completion.status)),
        ControlMessage::Error{message} => return Err(anyhow!("server error: {message}")),
        x => return Err(anyhow!("unexpected control response: {x:?}")),
    }

    write_frame(&mut s,&ControlMessage::Close).await?;
    println!("[SMOKE] remote control path PASSED");
    Ok(())
}



async fn probe_server(addr:&str)->Result<()> {
    let mut s=tokio::time::timeout(
        std::time::Duration::from_millis(800),
        TcpStream::connect(addr)
    ).await.map_err(|_| anyhow!("connect timeout"))??;
    s.set_nodelay(true)?;
    write_frame(&mut s,&ControlMessage::Hello{version:PROTOCOL_VERSION,client:"portbridge-probe".into()}).await?;
    match tokio::time::timeout(std::time::Duration::from_millis(800), recv(&mut s)).await {
        Ok(Ok(ControlMessage::Welcome{version,..})) if version == PROTOCOL_VERSION => {},
        Ok(Ok(x)) => return Err(anyhow!("unexpected probe response: {x:?}")),
        Ok(Err(e)) => return Err(e),
        Err(_) => return Err(anyhow!("probe response timeout")),
    }
    write_frame(&mut s,&ControlMessage::Ping).await?;
    match tokio::time::timeout(std::time::Duration::from_millis(800), recv(&mut s)).await {
        Ok(Ok(ControlMessage::Pong)) => {
            println!("PORTBRIDGE_OK {addr}");
            Ok(())
        }
        Ok(Ok(x)) => Err(anyhow!("unexpected ping response: {x:?}")),
        Ok(Err(e)) => Err(e),
        Err(_) => Err(anyhow!("ping timeout")),
    }
}



async fn safe_bulk_probe(addr:&str)->Result<()> {
    let mut s=TcpStream::connect(addr).await?;
    s.set_nodelay(true)?;
    write_frame(&mut s,&ControlMessage::Hello{version:PROTOCOL_VERSION,client:"portbridge-safe-bulk-probe".into()}).await?;
    match recv(&mut s).await? {
        ControlMessage::Welcome{..} => {},
        x => return Err(anyhow!("bad welcome: {x:?}")),
    }

    write_frame(&mut s,&ControlMessage::ListDevices).await?;
    let devices=match recv(&mut s).await? {
        ControlMessage::DeviceList{devices} => devices,
        x => return Err(anyhow!("bad device list: {x:?}")),
    };

    let d=devices.into_iter()
        .find(|d| matches!(d.mode, Some(UsbDeviceMode::AdbLike) | Some(UsbDeviceMode::XiaomiCandidate) | Some(UsbDeviceMode::Fastboot)))
        .ok_or_else(|| anyhow!("no Android/Xiaomi/Fastboot USB device found on remote PC"))?;

    println!("[BULK] selected {} {:04x}:{:04x} mode={:?} product={}",
        d.id,d.vid.unwrap_or(0),d.pid.unwrap_or(0),d.mode,d.product.clone().unwrap_or_default());

    write_frame(&mut s,&ControlMessage::OpenUsb{device_id:d.id.clone()}).await?;
    match recv(&mut s).await? {
        ControlMessage::UsbOpened{..} => println!("[BULK] remote open/claim: OK"),
        ControlMessage::Error{message} => return Err(anyhow!("remote open/claim failed: {message}")),
        x => return Err(anyhow!("unexpected open response: {x:?}")),
    }

    write_frame(&mut s,&ControlMessage::ProbeBulkEndpoints).await?;
    match recv(&mut s).await? {
        ControlMessage::BulkEndpointProbeResult{endpoints} => {
            if endpoints.is_empty() {
                println!("[BULK] no bulk endpoints detected");
            } else {
                for ep in endpoints {
                    println!("[BULK] endpoint=0x{:02x} dir={:?} open_ok={}{}",
                        ep.address,ep.direction,ep.open_ok,
                        ep.error.map(|e| format!(" error={e}")).unwrap_or_default());
                }
            }
            println!("[BULK] probe completed WITHOUT sending USB payloads");
        }
        ControlMessage::Error{message} => return Err(anyhow!("server error: {message}")),
        x => return Err(anyhow!("unexpected bulk probe response: {x:?}")),
    }
    write_frame(&mut s,&ControlMessage::Close).await?;
    Ok(())
}

#[tokio::main]
async fn main()->Result<()> {
    let args:Vec<String>=env::args().collect();
    if let Some(i)=args.iter().position(|a|a=="--usb-selftest") { let addr=args.get(i+1).map(String::as_str).unwrap_or("127.0.0.1:47990"); return selftest(addr).await; }
    if let Some(i)=args.iter().position(|a|a=="--list-remote-usb") { let addr=args.get(i+1).map(String::as_str).unwrap_or("127.0.0.1:47990"); return list_remote_usb(addr).await; }
    if let Some(i)=args.iter().position(|a|a=="--remote-control-smoke") { let addr=args.get(i+1).map(String::as_str).unwrap_or("127.0.0.1:47990"); return remote_control_smoke(addr).await; }
    if let Some(i)=args.iter().position(|a|a=="--probe-server") { let addr=args.get(i+1).map(String::as_str).unwrap_or("127.0.0.1:47990"); return probe_server(addr).await; }
    if let Some(i)=args.iter().position(|a|a=="--safe-bulk-probe") { let addr=args.get(i+1).map(String::as_str).unwrap_or("127.0.0.1:47990"); return safe_bulk_probe(addr).await; }
    eprintln!("Usage: portbridge-client --usb-selftest SERVER_IP:47990 | --list-remote-usb SERVER_IP:47990 | --remote-control-smoke SERVER_IP:47990 | --probe-server SERVER_IP:47990 | --safe-bulk-probe SERVER_IP:47990");
    Ok(())
}
