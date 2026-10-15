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

#[tokio::main]
async fn main()->Result<()> {
    let args:Vec<String>=env::args().collect();
    if let Some(i)=args.iter().position(|a|a=="--usb-selftest") { let addr=args.get(i+1).map(String::as_str).unwrap_or("127.0.0.1:47990"); return selftest(addr).await; }
    if let Some(i)=args.iter().position(|a|a=="--list-remote-usb") { let addr=args.get(i+1).map(String::as_str).unwrap_or("127.0.0.1:47990"); return list_remote_usb(addr).await; }
    eprintln!("Usage: portbridge-client --usb-selftest SERVER_IP:47990 | --list-remote-usb SERVER_IP:47990");
    Ok(())
}
