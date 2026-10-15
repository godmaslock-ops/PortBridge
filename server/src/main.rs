use anyhow::{anyhow, Context, Result};
use portbridge_common::*;
use std::{env, path::{Path, PathBuf}, process::Command, sync::Arc};
use tokio::{net::{TcpListener, TcpStream}, sync::Semaphore, time::{timeout, Duration}};
use uuid::Uuid;

fn mock_device() -> DeviceDescriptor {
    DeviceDescriptor { id:"mock:portbridge-usb".into(), vid:Some(0x1209), pid:Some(0x5042), manufacturer:Some("PortBridge".into()), product:Some("Mock USB Device".into()), serial:Some("PB-MOCK-0001".into()), instance_id:Some("MOCK\\PORTBRIDGE".into()) }
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
            instance_id:Some(instance)
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
    loop {
        let msg: ControlMessage = read_frame(&mut s).await?;
        match msg {
            ControlMessage::Ping => write_frame(&mut s,&ControlMessage::Pong).await?,
            ControlMessage::ListDevices => {
                let mut d = list_usb_windows().unwrap_or_default();
                if mock { d.push(mock_device()); }
                write_frame(&mut s,&ControlMessage::DeviceList{devices:d}).await?;
            }
            ControlMessage::OpenUsb{device_id} => {
                if mock {
                    opened=Some(device_id.clone());
                    write_frame(&mut s,&ControlMessage::UsbOpened{device_id}).await?;
                } else {
                    write_frame(&mut s,&ControlMessage::Error{message:"real USB open backend is not enabled yet; use --probe-adb for the current Windows validation stage".into()}).await?;
                }
            }
            ControlMessage::UsbRequest{request} => {
                if !mock {
                    write_frame(&mut s,&ControlMessage::Error{message:"real USB transfer backend is not enabled yet".into()}).await?;
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

    let mock=args.iter().any(|a|a=="--mock-usb");
    let addr=args.iter().find(|a|a.contains(':') && !a.starts_with("--")).cloned().unwrap_or_else(||"0.0.0.0:47990".into());
    let listener=TcpListener::bind(&addr).await?;
    println!("PortBridge server v0.10 listening on {addr} mock_usb={mock}");
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
