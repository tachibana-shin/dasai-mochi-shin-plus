//! Wi-Fi provisioning via a SoftAP + HTTP form.
//!
//! On boot the firmware always starts a SoftAP named [`AP_SSID`] (an open
//! network, no password) with a DHCP server, so a phone can join it and load the
//! provisioning page at `http://192.168.4.1/`. The page has a small form:
//! entering an SSID + password stores the credentials to flash (see
//! [`crate::storage::save_wifi_creds`]) and then software-reboots the device.
//!
//! On the next boot the station (STA) automatically connects to the saved
//! network, while the SoftAP stays up so the credentials can be changed again
//! later (just load the page again and submit a new network).
//!
//! If no credentials are stored yet, the STA simply stays idle (it retries
//! [`WifiController::connect_async`] in a loop and reports failures) — the SoftAP
//! remains available for setup either way.

use core::net::{Ipv4Addr, SocketAddr, SocketAddrV4};

use embassy_executor::Spawner;
use embassy_futures::select::Either;
use embassy_net::{
    IpListenEndpoint, Ipv4Cidr, Runner, StackResources, StaticConfigV4, tcp::TcpSocket,
};
use embassy_time::{Duration, Timer};
use esp_println::println;
use esp_radio::wifi::{
    AuthenticationMethodConfig, Config, ControllerConfig, Interface, WifiController,
    ap::AccessPointConfig, sta::StationConfig,
};

use crate::storage;

/// SSID broadcast by the provisioning SoftAP (open network, no password).
const AP_SSID: &str = "DasaiSetup";
/// Gateway / static IP of the provisioning SoftAP (clients reach HTTP here).
const GW_IP: Ipv4Addr = Ipv4Addr::new(192, 168, 4, 1);
/// Port the provisioning HTTP server listens on.
const HTTP_PORT: u16 = 80;

/// Bootstrap statics from the call site (mirrors the official example's helper).
macro_rules! mk_static {
    ($t:ty, $val:expr) => {{
        static STATIC_CELL: static_cell::StaticCell<$t> = static_cell::StaticCell::new();
        #[deny(unused_attributes)]
        let x = STATIC_CELL.uninit().write(($val));
        x
    }};
}

/// Initialise the Wi-Fi radio, the two network stacks (SoftAP + STA), and spawn
/// all network tasks. Called from `main` after `esp_rtos::start()`.
///
/// The CPU must be at 160 MHz for the radio real-time stack; the caller is
/// responsible for that (and for reserving a heap for esp-radio).
pub fn spawn(spawner: Spawner, wi_fi: esp_hal::peripherals::WIFI<'static>) {
    let saved = storage::load_wifi_creds();

    // Build the coex (AP + STA) config. If we have saved credentials, the STA
    // uses them (auto-connect); otherwise it stays with an empty/default config
    // and simply never associates.
    let station = match &saved {
        Some(creds) => StationConfig::default()
            .with_ssid(creds.ssid.as_bytes().try_into().unwrap())
            .with_authentication(AuthenticationMethodConfig::Wpa2Personal(
                creds.password.as_bytes().try_into().unwrap(),
            )),
        None => StationConfig::default(),
    };
    let access_point = AccessPointConfig::default().with_ssid(AP_SSID.try_into().unwrap());
    let initial_config = Config::AccessPointStation(station, access_point);

    println!("[wifi] starting radio, SoftAP `{AP_SSID}` at http://{}:{}", GW_IP, HTTP_PORT);
    if let Some(creds) = &saved {
        println!("[wifi] auto-connecting to network `{}`", creds.ssid);
    } else {
        println!("[wifi] no saved network: connect phone to `{AP_SSID}` to set one up");
    }

    let ap_device = Interface::access_point();
    let sta_device = Interface::station();
    let controller = WifiController::new(
        wi_fi,
        ControllerConfig::default().with_initial_config(initial_config),
    )
    .expect("failed to init esp-radio WifiController");

    let ap_config = embassy_net::Config::ipv4_static(StaticConfigV4 {
        address: Ipv4Cidr::new(GW_IP, 24),
        gateway: Some(GW_IP),
        dns_servers: Default::default(),
    });
    let sta_config = embassy_net::Config::dhcpv4(Default::default());

    let rng = esp_hal::rng::Rng::new();
    let seed = (rng.random() as u64) << 32 | rng.random() as u64;

    let (ap_stack, ap_runner) = embassy_net::new(
        ap_device,
        ap_config,
        mk_static!(StackResources<3>, StackResources::<3>::new()),
        seed,
    );
    let (_sta_stack, sta_runner) = embassy_net::new(
        sta_device,
        sta_config,
        mk_static!(StackResources<4>, StackResources::<4>::new()),
        seed,
    );

    spawner.spawn(connection(controller).unwrap());
    spawner.spawn(net_task(ap_runner).unwrap());
    spawner.spawn(net_task(sta_runner).unwrap());
    spawner.spawn(dhcp_task(ap_stack).unwrap());
    spawner.spawn(provision_server(ap_stack).unwrap());
}

/// Drive the radio: connect the STA to the saved network, and keep the SoftAP
/// alive. Retries STA connection on failure (e.g. network not in range yet).
#[embassy_executor::task]
async fn connection(mut controller: WifiController<'static>) -> ! {
    loop {
        match controller.connect_async().await {
            Ok(_connected) => {
                // Wait until either the STA drops, or an AP client joins/leaves.
                loop {
                    let ev = embassy_futures::select::select(
                        controller.wait_for_disconnect_async(),
                        controller.wait_for_access_point_connected_event_async(),
                    )
                    .await;
                    match ev {
                        Either::First(station_disconnected) => {
                            if let Ok(info) = station_disconnected {
                                println!("[wifi] STA disconnected: {:?}", info);
                                break;
                            }
                        }
                        Either::Second(event) => {
                            if let Ok(ev) = event {
                                match ev {
                                    esp_radio::wifi::ap::EventInfo::Connected(info) => {
                                        println!("[wifi] phone joined SoftAP: {:?}", info)
                                    }
                                    esp_radio::wifi::ap::EventInfo::Disconnected(info) => {
                                        println!("[wifi] phone left SoftAP: {:?}", info)
                                    }
                                }
                            }
                        }
                    }
                }
            }
            Err(e) => {
                println!("[wifi] STA connect failed: {e:?} (retrying)");
                Timer::after(Duration::from_secs(5)).await;
            }
        }
    }
}

/// Run one embassy-net stack in a loop until it settles.
#[embassy_executor::task(pool_size = 2)]
async fn net_task(mut runner: Runner<'static, Interface>) -> ! {
    runner.run().await
}

/// DHCP server on the SoftAP so a phone automatically gets an IP.
#[embassy_executor::task]
async fn dhcp_task(stack: embassy_net::Stack<'static>) -> ! {
    use edge_dhcp::{
        io::{self, DEFAULT_SERVER_PORT},
        server::{Server, ServerOptions},
    };
    use edge_nal::UdpBind;
    use edge_nal_embassy::{Udp, UdpBuffers};

    let ip = GW_IP;
    let mut buf = [0u8; 1500];
    let mut gw_buf = [Ipv4Addr::UNSPECIFIED];
    let buffers = UdpBuffers::<3, 1024, 1024, 10>::new();
    let unbound = Udp::new(stack, &buffers);
    let mut bound = unbound
        .bind(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, DEFAULT_SERVER_PORT)))
        .await
        .expect("failed to bind DHCP socket");

    loop {
        let _ = io::server::run(
            &mut Server::<_, 64>::new_with_et(ip),
            &ServerOptions::new(ip, Some(&mut gw_buf)),
            &mut bound,
            &mut buf,
        )
        .await
        .inspect_err(|e| println!("[wifi] DHCP server error: {e:?}"));
        Timer::after(Duration::from_millis(500)).await;
    }
}

/// Minimal HTTP server on the SoftAP: serves a provisioning form on `GET /` and
/// stores the submitted SSID/password on `POST /`, then reboots the device.
#[embassy_executor::task]
async fn provision_server(stack: embassy_net::Stack<'static>) -> ! {
    use embedded_io_async::Write;

    let mut rx = [0u8; 1024];
    let mut tx = [0u8; 1024];

    loop {
        let mut socket = TcpSocket::new(stack, &mut rx, &mut tx);
        socket.set_timeout(Some(Duration::from_secs(10)));

        if socket
            .accept(IpListenEndpoint {
                addr: None,
                port: HTTP_PORT,
            })
            .await
            .is_err()
        {
            continue;
        }

        // Read the request (headers + body) up to our buffer size.
        let mut buf = [0u8; 1024];
        let mut pos = 0;
        while pos < buf.len() {
            match socket.read(&mut buf[pos..]).await {
                Ok(0) => break,
                Ok(n) => {
                    pos += n;
                    // We have the full request once the body is fully read.
                    let hdr_end = header_end(&buf[..pos]).unwrap_or(pos);
                    let needed = hdr_end + content_length(&buf[..pos]);
                    if pos >= needed {
                        break;
                    }
                }
                Err(_) => break,
            }
        }

        let req = &buf[..pos];
        let is_post = req.starts_with(b"POST");
        let body = body_slice(req).unwrap_or(&[]);

        if is_post {
            // Parse `ssid=...&password=...` (form-urlencoded).
            let mut ssid = heapless::String::<32>::new();
            let mut password = heapless::String::<64>::new();
            parse_form(body, &mut ssid, &mut password);

            if !ssid.is_empty() && (password.is_empty() || password.len() >= 8) {
                let creds = storage::WifiCreds { ssid, password };
                storage::save_wifi_creds(&creds);
                println!("[wifi] credentials saved for `{}`, rebooting...", creds.ssid);
                let _ = socket.write_all(
                    b"HTTP/1.0 200 OK\r\nContent-Type: text/html\r\nConnection: close\r\n\r\n\
                      <html><body><h2>Saved! Rebooting now...</h2></body></html>",
                ).await;
                // Allow the response to flush before resetting.
                let _ = socket.flush().await;
                Timer::after(Duration::from_millis(200)).await;
                esp_hal::system::software_reset();
            } else {
                let _ = socket.write_all(
                    b"HTTP/1.0 400 Bad Request\r\nContent-Type: text/html\r\nConnection: close\r\n\r\n\
                      <html><body><h2>Invalid input</h2><p>Need a non-empty SSID and \
                      (for WPA2) a password of at least 8 characters.</p><a href='/'>Back</a></body></html>",
                ).await;
                let _ = socket.flush().await;
            }
        } else {
            let _ = socket.write_all(FORM_HTML).await;
            let _ = socket.flush().await;
        }

        socket.close();
        Timer::after(Duration::from_millis(200)).await;
    }
}

/// The provisioning HTML form (served on `GET /`).
const FORM_HTML: &[u8] = br#"HTTP/1.0 200 OK\r\nContent-Type: text/html\r\nConnection: close\r\n\r\n
<!DOCTYPE html>
<html>
<head><meta name="viewport" content="width=device-width, initial-scale=1">
<title>Dasai Mochi Shin - WiFi Setup</title>
<style>
  body { font-family: sans-serif; max-width: 360px; margin: 2em auto; padding: 0 1em; }
  input { width: 100%; padding: .7em; margin: .4em 0; box-sizing: border-box; font-size: 1em; }
  button { width: 100%; padding: .8em; font-size: 1em; background: #2b7; color: #fff; border: 0; }
  h1 { font-size: 1.3em; }
</style>
</head>
<body>
  <h1>Dasai Mochi Shin</h1>
  <p>Connect to your WiFi network. The device will save these and reboot to connect.</p>
  <form method="POST" action="/">
    <input name="ssid" placeholder="WiFi name (SSID)" autocomplete="off" required>
    <input name="password" type="password" placeholder="WiFi password" autocomplete="off">
    <button type="submit">Save &amp; connect</button>
  </form>
</body>
</html>
"#;

/// Locate the first occurrence of `needle` in `haystack`.
fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack
        .windows(needle.len())
        .position(|w| w == needle)
}

/// Byte index just past the `\r\n\r\n` separating headers from the body.
fn header_end(req: &[u8]) -> Option<usize> {
    find(req, b"\r\n\r\n").map(|i| i + 4)
}

/// The body (bytes after the header terminator).
fn body_slice(req: &[u8]) -> Option<&[u8]> {
    header_end(req).map(|e| &req[e..])
}

/// Parse the `Content-Length` header (returns 0 if absent or unparsable).
fn content_length(req: &[u8]) -> usize {
    let lower = req.to_ascii_lowercase();
    let mut i = 0;
    while i < lower.len() {
        if lower[i..].starts_with(b"content-length:") {
            let start = i + b"content-length:".len();
            let rest = &lower[start..];
            let end = rest.iter().position(|&b| b == b'\r').unwrap_or(rest.len());
            return core::str::from_utf8(&rest[..end])
                .ok()
                .and_then(|s| s.trim().parse().ok())
                .unwrap_or(0);
        }
        // Advance to the next line.
        match find(&lower[i..], b"\r\n") {
            Some(nl) => i += nl + 2,
            None => break,
        }
    }
    0
}

/// Parse `application/x-www-form-urlencoded` body into ssid + password.
fn parse_form(body: &[u8], ssid: &mut heapless::String<32>, password: &mut heapless::String<64>) {
    for field in body.split(|&b| b == b'&') {
        let mut parts = field.splitn(2, |&b| b == b'=');
        let k = parts.next().unwrap_or(&[]);
        let v = parts.next().unwrap_or(&[]);
        match k {
            b"ssid" => *ssid =
                heapless::String::from_utf8(url_decode::<32>(v)).unwrap_or_default(),
            b"password" => *password =
                heapless::String::from_utf8(url_decode::<64>(v)).unwrap_or_default(),
            _ => {}
        }
    }
}

/// Decode a form-urlencoded value (`+` → space, `%XX` → byte). Output capacity is
/// `N` bytes and the decode is capped at that many bytes.
fn url_decode<const N: usize>(input: &[u8]) -> heapless::Vec<u8, N> {
    let mut out: heapless::Vec<u8, N> = heapless::Vec::new();
    let mut i = 0;
    while i < input.len() {
        let b = input[i];
        match b {
            b'+' => {
                let _ = out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < input.len() => {
                // Decode two hex digits if present.
                if let (Some(hi), Some(lo)) = (hex_val(input[i + 1]), hex_val(input[i + 2])) {
                    let _ = out.push((hi << 4) | lo);
                    i += 3;
                } else {
                    let _ = out.push(b'%');
                    i += 1;
                }
            }
            _ => {
                let _ = out.push(b);
                i += 1;
            }
        }
    }
    out
}

/// Hex digit (0-9a-fA-F) to its value.
fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}
