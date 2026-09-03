//! Wi-Fi provisioning via SoftAP + captive-portal HTTP configuration.
//!
//! On boot the firmware always starts a SoftAP named [`AP_SSID`] (an open
//! network, no password) with a DHCP server, so a phone can join it. A captive
//! portal then drives the setup:
//!
//! - **DNS redirect** (`dns_task`): answers every DNS query with `192.168.4.1`
//!   so any host a phone tries lands on the device.
//! - **HTTP intercept**: the HTTP server answers any request whose `Host` is not
//!   `192.168.4.1` with a `302 Found` redirect to `http://192.168.4.1/`, so a
//!   browser that opens any page gets pushed to the setup screen.
//! - **Setup UI**: the page performs a real Wi-Fi scan (via the radio) and shows
//!   the detected networks as a list (like a phone's Wi-Fi settings screen):
//!   name, signal strength and a lock icon for secured networks. The user picks
//!   a network and types its password; the device saves credentials to flash and
//!   reboots.
//!
//! After reboot the STA auto-connects to the saved network while the SoftAP
//! stays up for later re-provisioning.

use core::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use core::sync::atomic::{AtomicU8, Ordering};

use embassy_executor::Spawner;
use embassy_net::{
    IpListenEndpoint, Ipv4Cidr, Runner, StackResources, StaticConfigV4, tcp::TcpSocket,
};
use embassy_time::{Duration, Timer};
use esp_println::println;
use esp_radio::wifi::{
    AuthenticationMethod, AuthenticationMethodConfig, Config, ControllerConfig, Interface,
    WifiController,
    ap::{AccessPointConfig, AccessPointInfo},
    scan::ScanConfig,
    sta::StationConfig,
};
use heapless::Vec as HeaplessVec;

use crate::storage;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const AP_SSID: &str = "DasaiSetup";
const GW_IP: Ipv4Addr = Ipv4Addr::new(192, 168, 4, 1);
const GW_IP_STR: &[u8] = b"192.168.4.1";
const HTTP_PORT: u16 = 80;
const MAX_APS: usize = 40;

// ---------------------------------------------------------------------------
// Scan coordination — atomic flags + shared buffer (no embassy-sync channels)
// ---------------------------------------------------------------------------

/// 0 = idle, 1 = scan requested, 2 = scan complete
static SCAN_STATE: AtomicU8 = AtomicU8::new(0);

/// Shared scan result buffer.  Written by `wifi_driver`, read by
/// `provision_server`.  The write happens-before the atomic store to
/// `SCAN_STATE = 2`, and the read happens-after the load of `SCAN_STATE == 2`,
/// so the data is visible without a mutex (single-threaded executor).
type ScanResult = HeaplessVec<AccessPointInfo, MAX_APS>;

// Safe: ESP32-C3 is single-threaded; access is sequenced via atomic flags.
#[allow(static_mut_refs)]
static mut SCAN_RESULT: ScanResult = HeaplessVec::new();

const STATE_IDLE: u8 = 0;
const STATE_REQUESTED: u8 = 1;
const STATE_READY: u8 = 2;

// ---------------------------------------------------------------------------
// Static helper macro
// ---------------------------------------------------------------------------

macro_rules! mk_static {
    ($t:ty, $val:expr) => {{
        static STATIC_CELL: static_cell::StaticCell<$t> = static_cell::StaticCell::new();
        #[deny(unused_attributes)]
        let x = STATIC_CELL.uninit().write(($val));
        x
    }};
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

pub fn spawn(spawner: Spawner, wi_fi: esp_hal::peripherals::WIFI<'static>) {
    let saved = storage::load_wifi_creds();

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

    println!("[wifi] starting radio, SoftAP `{AP_SSID}` at http://{}", GW_IP);
    if let Some(creds) = &saved {
        println!("[wifi] auto-connecting to `{}`", creds.ssid);
    } else {
        println!("[wifi] no saved network: join `{AP_SSID}` and open any page");
    }

    let ap_device = Interface::access_point();
    let sta_device = Interface::station();
    let controller = WifiController::new(
        wi_fi,
        ControllerConfig::default().with_initial_config(initial_config),
    )
    .expect("failed to init WifiController");

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
        mk_static!(StackResources<5>, StackResources::<5>::new()),
        seed,
    );
    let (_sta_stack, sta_runner) = embassy_net::new(
        sta_device,
        sta_config,
        mk_static!(StackResources<4>, StackResources::<4>::new()),
        seed,
    );

    spawner.spawn(wifi_driver(controller).unwrap());
    spawner.spawn(net_task(ap_runner).unwrap());
    spawner.spawn(net_task(sta_runner).unwrap());
    spawner.spawn(dhcp_task(ap_stack).unwrap());
    spawner.spawn(dns_task(ap_stack).unwrap());
    spawner.spawn(provision_server(ap_stack).unwrap());
}

// ---------------------------------------------------------------------------
// Wi-Fi driver task — handles STA connect + scan requests
// ---------------------------------------------------------------------------

#[embassy_executor::task]
async fn wifi_driver(mut controller: WifiController<'static>) -> ! {
    loop {
        // Serve scan requests from the HTTP server.
        if SCAN_STATE.load(Ordering::Acquire) == STATE_REQUESTED {
            let mut list: ScanResult = HeaplessVec::new();
            if let Ok(aps) = controller.scan_async(&ScanConfig::default()).await {
                for ap in aps {
                    if list.push(ap).is_err() {
                        break;
                    }
                }
            }
            // Publish result.
            // SAFETY: single-threaded executor, sequenced by atomic flag.
            #[allow(static_mut_refs)]
            unsafe {
                SCAN_RESULT = list;
            }
            SCAN_STATE.store(STATE_READY, Ordering::Release);
            continue;
        }

        // Normal STA operation: try to connect, reconnect on failure.
        match controller.connect_async().await {
            Ok(_) => {
                let _ = controller.wait_for_disconnect_async().await;
            }
            Err(_) => {
                Timer::after(Duration::from_millis(500)).await;
            }
        }
    }
}

#[embassy_executor::task(pool_size = 2)]
async fn net_task(mut runner: Runner<'static, Interface>) -> ! {
    runner.run().await
}

// ---------------------------------------------------------------------------
// DHCP server
// ---------------------------------------------------------------------------

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
        .bind(SocketAddr::V4(SocketAddrV4::new(
            Ipv4Addr::UNSPECIFIED,
            DEFAULT_SERVER_PORT,
        )))
        .await
        .expect("bind DHCP");

    loop {
        let _ = io::server::run(
            &mut Server::<_, 64>::new_with_et(ip),
            &ServerOptions::new(ip, Some(&mut gw_buf)),
            &mut bound,
            &mut buf,
        )
        .await
        .inspect_err(|e| println!("[wifi] DHCP error: {e:?}"));
        Timer::after(Duration::from_millis(500)).await;
    }
}

// ---------------------------------------------------------------------------
// Captive-portal DNS: answers every query with GW_IP.
// ---------------------------------------------------------------------------

#[embassy_executor::task]
async fn dns_task(stack: embassy_net::Stack<'static>) -> ! {
    use edge_nal::{UdpBind, UdpReceive, UdpSend};
    use edge_nal_embassy::{Udp, UdpBuffers};

    let buffers = UdpBuffers::<2, 512, 512, 4>::new();
    let udp = Udp::new(stack, &buffers);
    let mut bound = udp
        .bind(SocketAddr::V4(SocketAddrV4::new(
            Ipv4Addr::UNSPECIFIED,
            53,
        )))
        .await
        .expect("bind DNS");

    let mut req = [0u8; 512];
    let mut resp = [0u8; 512];

    loop {
        let (len, peer) = match bound.receive(&mut req).await {
            Ok(x) => x,
            Err(_) => continue,
        };
        let n = build_dns_response(&req[..len], &mut resp);
        if n > 0 {
            let _ = bound.send(peer, &resp[..n]).await;
        }
    }
}

fn build_dns_response(req: &[u8], out: &mut [u8]) -> usize {
    if req.len() < 12 {
        return 0;
    }
    // Copy transaction ID and set response flags.
    out[0..2].copy_from_slice(&req[0..2]);
    out[2] = 0x81;
    out[3] = 0x80;
    out[4..6].copy_from_slice(&[0x00, 0x01]); // QDCOUNT
    out[6..8].copy_from_slice(&[0x00, 0x01]); // ANCOUNT
    out[8..12].fill(0);

    // Walk the QNAME (label sequence terminated by 0x00).
    let qstart = 12;
    let mut i = qstart;
    loop {
        if i >= req.len() {
            return 0;
        }
        let lenb = req[i];
        if lenb == 0 {
            i += 1;
            break;
        }
        if lenb & 0xC0 == 0xC0 {
            i += 2;
            break;
        }
        i += 1 + lenb as usize;
    }
    let qname_len = i - qstart;
    if 12 + qname_len + 4 > out.len() {
        return 0;
    }
    // Echo question.
    out[12..12 + qname_len].copy_from_slice(&req[qstart..i]);
    let mut pos = 12 + qname_len;
    if i + 4 <= req.len() {
        out[pos..pos + 4].copy_from_slice(&req[i..i + 4]); // QTYPE + QCLASS
        pos += 4;
    }
    // Answer: pointer to echoed name, type A, class IN, TTL 60s, GW_IP.
    out[pos..pos + 2].copy_from_slice(&[0xC0, 0x0C]);
    pos += 2;
    out[pos..pos + 2].copy_from_slice(&[0x00, 0x01]); // TYPE A
    pos += 2;
    out[pos..pos + 2].copy_from_slice(&[0x00, 0x01]); // CLASS IN
    pos += 2;
    out[pos..pos + 4].copy_from_slice(&[0x00, 0x00, 0x00, 0x3C]); // TTL 60
    pos += 4;
    out[pos..pos + 2].copy_from_slice(&[0x00, 0x04]); // RDLENGTH 4
    pos += 2;
    out[pos..pos + 4].copy_from_slice(&GW_IP.octets());
    pos += 4;
    pos
}

// ---------------------------------------------------------------------------
// HTTP provision server with scan-driven Wi-Fi list + captive-portal intercept.
// ---------------------------------------------------------------------------

#[embassy_executor::task]
async fn provision_server(stack: embassy_net::Stack<'static>) -> ! {
    use embedded_io_async::Write;

    let mut rx = [0u8; 2048];
    let mut tx = [0u8; 4096];

    loop {
        let mut socket = TcpSocket::new(stack, &mut rx, &mut tx);
        socket.set_timeout(Some(Duration::from_secs(15)));

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

        // Read request (headers + body).
        let mut buf = [0u8; 1024];
        let mut pos = 0;
        while pos < buf.len() {
            match socket.read(&mut buf[pos..]).await {
                Ok(0) => break,
                Ok(n) => {
                    pos += n;
                    let hdr_end = header_end(&buf[..pos]).unwrap_or(pos);
                    if pos >= hdr_end + content_length(&buf[..pos]) {
                        break;
                    }
                }
                Err(_) => break,
            }
        }

        let req = &buf[..pos];
        let is_post = req.starts_with(b"POST");

        if is_post {
            let mut ssid = heapless::String::<32>::new();
            let mut password = heapless::String::<64>::new();
            parse_form(body_slice(req).unwrap_or(&[]), &mut ssid, &mut password);

            if !ssid.is_empty() && (password.is_empty() || password.len() >= 8) {
                let creds = storage::WifiCreds { ssid, password };
                storage::save_wifi_creds(&creds);
                println!("[wifi] saved `{}`, rebooting...", creds.ssid);
                let _ = socket.write_all(RESP_SAVED.as_bytes()).await;
                let _ = socket.flush().await;
                Timer::after(Duration::from_millis(200)).await;
                esp_hal::system::software_reset();
            } else {
                let _ = socket.write_all(RESP_BAD.as_bytes()).await;
                let _ = socket.flush().await;
            }
        } else if !is_local_host(req) {
            // Captive portal: redirect any other host to our setup page.
            let _ = socket.write_all(REDIRECT).await;
        } else {
            // Local request: serve the scan-driven Wi-Fi list.
            let scan = request_scan().await;
            let _ = socket.write_all(HTTP_200).await;
            serve_wifi_list(&mut socket, &scan).await;
        }
        let _ = socket.flush().await;
        socket.close();
        Timer::after(Duration::from_millis(150)).await;
    }
}

/// Request a Wi-Fi scan from the driver task and wait for the result.
async fn request_scan() -> ScanResult {
    SCAN_STATE.store(STATE_REQUESTED, Ordering::Release);
    // Poll until the driver publishes the result.
    while SCAN_STATE.load(Ordering::Acquire) != STATE_READY {
        Timer::after(Duration::from_millis(10)).await;
    }
    SCAN_STATE.store(STATE_IDLE, Ordering::Release);
    // SAFETY: sequenced by atomic flag; only one reader.
    #[allow(static_mut_refs)]
    unsafe {
        core::ptr::read(&SCAN_RESULT as *const ScanResult)
    }
}

/// Serve the Wi-Fi list HTML page.
async fn serve_wifi_list(socket: &mut TcpSocket<'_>, scan: &ScanResult) {
    use embedded_io_async::Write;

    let _ = socket.write_all(PAGE_HEAD.as_bytes()).await;

    if scan.is_empty() {
        let _ = socket
            .write_all(
                "<div class=\"em\">Không tìm thấy mạng nào.<br>\
                 <a href=\"/\" style=\"color:#6aa9ff\">Quét lại</a></div>"
                    .as_bytes(),
            )
            .await;
    } else {
        for ap in scan.iter() {
            let name = escape_html(ap.ssid.as_str().as_bytes());
            let bars = signal_bars(ap.signal_strength);
            let locked = ap.auth_method != Some(AuthenticationMethod::None);

            let _ = socket
                .write_all(b"<label class=\"n\"><input type=\"radio\" name=\"ssid\" value=\"")
                .await;
            let _ = socket.write_all(&name).await;
            let _ = socket.write_all(b"\">").await;
            let _ = socket.write_all(bars).await;
            let _ = socket.write_all(b"<span class=\"nm\">").await;
            let _ = socket.write_all(&name).await;
            let _ = socket.write_all(b"</span>").await;
            if locked {
                // Lock icon (Unicode 🔒 rendered as HTML entity).
                let _ = socket.write_all(b"<span class=\"lk\">&#128274;</span>").await;
            }
            let _ = socket.write_all(b"</label>\n").await;
        }
    }

    let _ = socket.write_all(PAGE_TAIL.as_bytes()).await;
}

// ---------------------------------------------------------------------------
// Response constants
// ---------------------------------------------------------------------------

const HTTP_200: &[u8] =
    b"HTTP/1.0 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nConnection: close\r\n\r\n";

const REDIRECT: &[u8] = b"HTTP/1.0 302 Found\r\n\
    Location: http://192.168.4.1/\r\n\
    Content-Length: 0\r\n\
    Connection: close\r\n\
    \r\n";

const RESP_SAVED: &str = "HTTP/1.0 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nConnection: close\r\n\r\n\
    <!DOCTYPE html><html><body style='font-family:sans-serif;text-align:center;margin-top:20vh'>\
    <h2>Đã lưu! Đang khởi động lại...</h2></body></html>";

const RESP_BAD: &str = "HTTP/1.0 400 Bad Request\r\nContent-Type: text/html; charset=utf-8\r\nConnection: close\r\n\r\n\
    <!DOCTYPE html><html><body style='font-family:sans-serif'>\
    <h2>Dữ liệu không hợp lệ</h2><p>Cần chọn mạng và nhập mật khẩu (>= 8 ký tự cho WPA2).</p>\
    <a href='/'>Quay lại</a></body></html>";

// ---------------------------------------------------------------------------
// Wi-Fi list page (HTML split to avoid giant const)
// ---------------------------------------------------------------------------

const PAGE_HEAD: &str = "\
<!DOCTYPE html>
<html lang=\"vi\">
<head>
<meta charset=\"utf-8\">
<meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">
<title>Dasai Mochi Shin - WiFi</title>
<style>
*{box-sizing:border-box;font-family:system-ui,sans-serif}
body{margin:0;background:#0f0f13;color:#eee;min-height:100vh}
.h{padding:18px 16px;border-bottom:1px solid #26262e}
.h h1{font-size:17px;margin:0;font-weight:600;display:inline-block}
.rf{float:right;color:#6aa9ff;font-size:12px;text-decoration:none;line-height:22px}
.ht{padding:12px 16px 4px;font-size:13px;color:#9a9aa5}
.n{display:flex;align-items:center;gap:12px;padding:14px 16px;border-bottom:1px solid #1c1c22;cursor:pointer}
.n input{display:none}
.n:has(input:checked){background:#17251d}
.g{height:7px;border-radius:1px;background:#8fd19e;display:inline-block;margin-right:1px}
.nm{flex:1;font-size:14px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}
.lk{color:#b3b3bd;font-size:13px}
.pw{padding:16px}
.pw input{width:100%;padding:13px;border-radius:10px;border:1px solid #2c2c36;background:#17171d;color:#eee;font-size:15px}
.bt{margin:0 16px;width:calc(100% - 32px);padding:14px;border:0;border-radius:12px;background:#2ea36b;color:#fff;font-size:16px;font-weight:600}
.em{padding:26px 16px;text-align:center;color:#777;font-size:14px}
</style>
</head>
<body>
<div class=\"h\"><h1>Wi-Fi</h1><a class=\"rf\" href=\"/\">Quét lại</a></div>
<form method=\"POST\" action=\"/\">
<p class=\"ht\">Chọn mạng Wi-Fi rồi nhập mật khẩu</p>
";

const PAGE_TAIL: &str = "
<div class=\"pw\">
<input type=\"password\" name=\"password\" placeholder=\"Nhập mật khẩu Wi-Fi\" autocomplete=\"off\">
</div>
<button type=\"submit\" class=\"bt\">Kết nối</button>
</form>
</body>
</html>
";

// ---------------------------------------------------------------------------
// HTML / HTTP helpers
// ---------------------------------------------------------------------------

fn signal_bars(signal: i8) -> &'static [u8] {
    match signal {
        s if s >= -50 => b"<span class=\"g\"></span><span class=\"g\"></span><span class=\"g\"></span><span class=\"g\"></span>",
        s if s >= -65 => b"<span class=\"g\"></span><span class=\"g\"></span><span class=\"g\"></span>",
        s if s >= -75 => b"<span class=\"g\"></span><span class=\"g\"></span>",
        s if s >= -85 => b"<span class=\"g\"></span>",
        _ => b"<span class=\"g\" style=\"opacity:.3\"></span>",
    }
}

fn escape_html(s: &[u8]) -> heapless::Vec<u8, 224> {
    let mut out = heapless::Vec::<u8, 224>::new();
    for &b in s {
        let rep: &[u8] = match b {
            b'&' => b"&amp;",
            b'<' => b"&lt;",
            b'>' => b"&gt;",
            b'"' => b"&quot;",
            b'\'' => b"&#39;",
            _ => {
                let _ = out.push(b);
                continue;
            }
        };
        for &c in rep {
            let _ = out.push(c);
        }
    }
    out
}

/// Check whether the request's `Host` header points to our own IP.
fn is_local_host(req: &[u8]) -> bool {
    let lower = req.to_ascii_lowercase();
    for line in lower.split(|&b| b == b'\n') {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        if let Some(rest) = line.strip_prefix(b"host:") {
            let mut it = rest.iter();
            while matches!(it.clone().next(), Some(b' ') | Some(b'\t')) {
                it.next();
            }
            return it.as_slice().starts_with(GW_IP_STR);
        }
    }
    true // no Host header → treat as local
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack.windows(needle.len()).position(|w| w == needle)
}

fn header_end(req: &[u8]) -> Option<usize> {
    find(req, b"\r\n\r\n").map(|i| i + 4)
}

fn body_slice(req: &[u8]) -> Option<&[u8]> {
    header_end(req).map(|e| &req[e..])
}

fn content_length(req: &[u8]) -> usize {
    let lower = req.to_ascii_lowercase();
    let mut i = 0;
    while i < lower.len() {
        if lower[i..].starts_with(b"content-length:") {
            let start = i + b"content-length:".len();
            let rest = &lower[start..];
            let end = rest
                .iter()
                .position(|&b| b == b'\r')
                .unwrap_or(rest.len());
            return core::str::from_utf8(&rest[..end])
                .ok()
                .and_then(|s| s.trim().parse().ok())
                .unwrap_or(0);
        }
        match find(&lower[i..], b"\r\n") {
            Some(nl) => i += nl + 2,
            None => break,
        }
    }
    0
}

fn parse_form(
    body: &[u8],
    ssid: &mut heapless::String<32>,
    password: &mut heapless::String<64>,
) {
    for field in body.split(|&b| b == b'&') {
        let mut parts = field.splitn(2, |&b| b == b'=');
        let k = parts.next().unwrap_or(&[]);
        let v = parts.next().unwrap_or(&[]);
        match k {
            b"ssid" => {
                *ssid =
                    heapless::String::from_utf8(url_decode::<32>(v)).unwrap_or_default()
            }
            b"password" => {
                *password =
                    heapless::String::from_utf8(url_decode::<64>(v)).unwrap_or_default()
            }
            _ => {}
        }
    }
}

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
                if let (Some(hi), Some(lo)) = (hex_val(input[i + 1]), hex_val(input[i + 2]))
                {
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

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}
