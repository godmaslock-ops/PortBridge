# PortBridge MVP v0.9

Experimental Rust core for a future Windows-to-Windows USB-over-network tool.

## What works now

- Versioned TCP control protocol and session IDs.
- USB device descriptors and list/open commands.
- USB request/completion contract for control/bulk/interrupt/isochronous transfers.
- Mock USB server + client self-test.
- Windows USB discovery test using present PnP USB devices.
- One-click BAT helpers.
- GitHub Actions Linux/Windows checks and Windows x64 release artifact.

## Important boundary

This is **not yet full USB redirection**. PC2 cannot yet enumerate the remote phone as a local USB device. The remaining hard milestone is a Windows export backend plus a virtual USB import/controller driver. That portion must be developed and tested on Windows.

## Recommended first Windows test

Use GitHub Actions to produce the artifact `portbridge-windows-x64-v0.9`. Put the downloaded files together on PC1 and double-click `test_usb_on_pc1.bat` with a phone connected. Send back `usb_test_log.txt` or a photo of the console.

## Mock protocol test

On one PC, run `run_mock_server.bat`. On another PC, run `run_mock_selftest.bat` and enter the server IP. Expected result: `USB self-test passed`.

## Next milestone after PC1 discovery passes

1. Real Windows USB export backend using WinUSB/USB device handles where possible.
2. Driver strategy for devices that are not WinUSB-bound.
3. Client-side virtual USB host controller / import backend.
4. Validation order: ADB -> Fastboot -> Samsung Download/Odin -> Xiaomi service modes.
