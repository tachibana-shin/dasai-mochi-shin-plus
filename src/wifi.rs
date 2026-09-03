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
//! - **Setup UI**: `GET /` serves an SSID + password form with a "Quét lại"
//!   button; `GET /scan` runs a background Wi-Fi scan; `POST /` saves the
//!   credentials to flash and connects immediately (no reboot).
//!
//! After saving credentials the STA connects to the network right away while
//! the SoftAP stays up for later re-provisioning.

use core::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use core::sync::atomic::{AtomicU8, Ordering};

use embassy_executor::Spawner;
use embassy_net::{
    IpListenEndpoint, Ipv4Cidr, Runner, Stack, StackResources, StaticConfigV4, dns::DnsSocket,
    tcp::TcpSocket,
};
use embassy_time::{Duration, Timer};
use esp_println::println;
use esp_radio::wifi::{
    AuthenticationMethod, AuthenticationMethodConfig, Config, ControllerConfig, Interface,
    WifiController, PowerSaveMode,
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
// NTP settings
// ---------------------------------------------------------------------------
/// Hostname resolved (via the STA's DHCP-provided DNS) to get the time from
/// Google's public NTP anycast pool.
const NTP_HOST: &str = "time.google.com";
/// Service port (NTP).
const NTP_PORT: u16 = 123;
/// Seconds between 1900-01-01 (NTP epoch) and 1970-01-01 (Unix epoch).
const NTP_UNIX_OFFSET: u64 = 2_208_988_800;
/// How often we (re)sync the clock while connected. Kept short (1 min) so a
/// failed sync — e.g. the first attempt racing DHCP, or a network that briefly
/// drops NTP packets — is retried promptly rather than after an hour.
const NTP_REFRESH: Duration = Duration::from_secs(60);
/// If the primary hostname can't be reached, fall back to these fixed public
/// NTP servers (pool.ntp.org is aggressive about blocking port 123 abuse, so
/// only used as a last resort).
const NTP_FALLBACKS: &[Ipv4Addr] = &[
    Ipv4Addr::new(129, 6, 15, 28),  // time-a-g.nist.gov
    Ipv4Addr::new(162, 159, 200, 123), // time.cloudflare.com
];

/// Hard cap on how long a scan may take before we give up (avoids hanging the
/// driver when the radio is busy serving the SoftAP).
const SCAN_TIMEOUT: Duration = Duration::from_secs(6);
/// How long `wifi_driver` may spend on a STA connect before dropping back to
/// check for scan / reconfigure requests.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(8);

// ---------------------------------------------------------------------------
// Cross-task coordination — atomic flags + shared buffers.
//
// All traffic happens on a single-threaded executor, so plain statics guarded
// by atomic "happens-before" flags are safe and avoid any channel/critical-
// section dependency (which froze at boot on this RISC-V chip).
// ---------------------------------------------------------------------------

/// 1 = HTTP server wants a fresh scan.  Cleared by the driver when done.
static SCAN_REQUEST: AtomicU8 = AtomicU8::new(0);
/// 1 = latest scan result is available in [`SCAN_CACHE`].
static SCAN_READY: AtomicU8 = AtomicU8::new(0);
/// 1 = HTTP just saved new credentials; driver must re-apply station config.
static NET_UPDATE: AtomicU8 = AtomicU8::new(0);
/// 1 = user disabled Wi-Fi from the settings screen; the driver idles instead
/// of trying to connect to any network. Set/cleared via [`set_wifi_enabled`].
static WIFI_ENABLED: AtomicU8 = AtomicU8::new(1);
/// 1 = user asked to force the SoftAP provisioning mode back up so they can
/// join it with a phone and pick/change the network. Cleared by the driver
/// when the AP is brought up.
static FORCE_AP: AtomicU8 = AtomicU8::new(0);

/// Public on/off control used by the settings screen. `true` keeps the STA
/// connect loop running; `false` makes the driver idle (STA off, no connect).
pub fn set_wifi_enabled(on: bool) {
    WIFI_ENABLED.store(on as u8, Ordering::Release);
    println!("[wifi] user set Wi-Fi {}", if on { "ON" } else { "OFF" });
}

/// Current user Wi-Fi on/off state (see [`set_wifi_enabled`]).
pub fn wifi_enabled() -> bool {
    WIFI_ENABLED.load(Ordering::Acquire) != 0
}

/// Ask the driver to bring the SoftAP provisioning mode back up so the user
/// can join `DasaiSetup` and connect to a different network.
pub fn request_force_ap() {
    FORCE_AP.store(1, Ordering::Release);
    println!("[wifi] force-AP requested by user");
}

type ScanResult = HeaplessVec<AccessPointInfo, MAX_APS>;

// Single-threaded executor → safe as long as access is sequenced via the
// atomic flags above.
#[allow(static_mut_refs)]
static mut SCAN_CACHE: ScanResult = HeaplessVec::new();

/// Constant-style "set if 1 then clear" (atomic exchange wrapper for targets
/// where `AtomicU8::swap` isn't available).  Returns the previous value.
fn take_flag(flag: &AtomicU8) -> bool {
    // Single-threaded executor: no true concurrent writers, so a simple
    // load-and-store is sufficient and avoids unavailable RMW intrinsics.
    let was_set = flag.load(Ordering::Acquire) == 1;
    flag.store(0, Ordering::Release);
    was_set
}

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
    let creds = storage::load_all_wifi_creds();

    let has_creds = !creds.is_empty();
    let initial_station = match creds.first() {
        Some(c) => station_from(c),
        None => StationConfig::default(),
    };
    let access_point = AccessPointConfig::default().with_ssid(AP_SSID.try_into().unwrap());
    // The SoftAP only turns on when there are no saved credentials (fresh setup).
    // Once credential exist we boot straight into STA mode only — no AP.
    // When there are no creds yet we bring up the AP *and* an (empty) station
    // interface (APSTA mode): the radio must be in a combined mode for a Wi-Fi
    // scan to actually work on this single-radio chip while serving the SoftAP.
    let empty_station = StationConfig::default();
    let initial_config = if has_creds {
        Config::Station(initial_station)
    } else {
        Config::AccessPointStation(empty_station, access_point.clone())
    };

    if has_creds {
        println!("[wifi] {} saved network(s): STA-only, no SoftAP", creds.len());
        for (i, c) in creds.iter().enumerate() {
            println!("[wifi]   {}. {}", i + 1, c.ssid);
        }
    } else {
        println!("[wifi] no saved network: SoftAP `{AP_SSID}` at http://{}", GW_IP);
    }

    let ap_device = Interface::access_point();
    let sta_device = Interface::station();
    let mut controller = WifiController::new(
        wi_fi,
        ControllerConfig::default().with_initial_config(initial_config),
    )
    .expect("failed to init WifiController");

    // Power / heat optimisation (esp-radio `unstable` API):
    // - Modem-sleep: the radio sleeps between beacons instead of listening
    //   continuously, which cuts idle current noticeably.
    // - Lower max TX power: the board is close to the router and doesn't need
    //   full 20 dBm; 9 dBm (36 * 0.25 dBm) still reaches it while emitting far
    //   less heat.
    let _ = controller.set_power_saving(PowerSaveMode::Minimum);
    let _ = controller.set_max_tx_power(36);

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
    let (sta_stack, sta_runner) = embassy_net::new(
        sta_device,
        sta_config,
        mk_static!(StackResources<5>, StackResources::<5>::new()),
        seed,
    );

    spawner.spawn(wifi_driver(controller, !has_creds).unwrap());
    spawner.spawn(net_task(ap_runner).unwrap());
    spawner.spawn(net_task(sta_runner).unwrap());
    spawner.spawn(dhcp_task(ap_stack).unwrap());
    spawner.spawn(dns_task(ap_stack).unwrap());
    spawner.spawn(ntp_task(sta_stack).unwrap());
    spawner.spawn(provision_server(ap_stack).unwrap());
}

// ---------------------------------------------------------------------------
// Wi-Fi driver — owns the controller, serves scans + reconfiguration, and
// keeps the STA connected.  Runs on the single-threaded executor.
// ---------------------------------------------------------------------------

fn station_from(creds: &storage::WifiCreds) -> StationConfig {
    StationConfig::default()
        .with_ssid(creds.ssid.as_bytes().try_into().unwrap())
        .with_authentication(AuthenticationMethodConfig::Wpa2Personal(
            creds.password.as_bytes().try_into().unwrap(),
        ))
}

/// Build the Wi-Fi `Config` for a given network, keeping the SoftAP up only
/// while it is still enabled (provisioning).
fn net_config(ap_enabled: bool, creds: &storage::WifiCreds) -> Config {
    let station = station_from(creds);
    if ap_enabled {
        let ap = AccessPointConfig::default().with_ssid(AP_SSID.try_into().unwrap());
        Config::AccessPointStation(station, ap)
    } else {
        Config::Station(station)
    }
}

#[embassy_executor::task]
async fn wifi_driver(mut controller: WifiController<'static>, ap_enabled_initial: bool) -> ! {
    // Starts with the SoftAP up (provisioning mode).  Once the STA connects to a
    // real network we power the SoftAP down so the radio is free.
    let mut ap_enabled = ap_enabled_initial;
    // Set when the user requests re-provisioning from the settings screen; while
    // set we serve only the SoftAP (no STA connect) until new credentials arrive.
    let mut force_ap_mode = false;
    let mut net_list = storage::load_all_wifi_creds();
    let mut idx = 0usize;

    loop {
        // 0) User controls from the settings screen.
        if take_flag(&FORCE_AP) {
            // Bring the SoftAP provisioning mode back up so the user can join
            // `DasaiSetup` with a phone and change the network.
            println!("[wifi] forcing SoftAP provisioning mode");
            let _ = controller.set_config(&Config::AccessPointStation(
                StationConfig::default(),
                AccessPointConfig::default().with_ssid(AP_SSID.try_into().unwrap()),
            ));
            ap_enabled = true;
            force_ap_mode = true;
        }

        // If the user disabled Wi-Fi, idle (keep whatever radio state we have)
        // instead of scanning / retrying the STA. We still poll the control
        // flags so a "re-enable" or "force AP" is picked up promptly.
        if !wifi_enabled() {
            Timer::after(Duration::from_millis(200)).await;
            continue;
        }

        // 1) Provisioning-only work while the SoftAP is still up.
        if ap_enabled {
            if take_flag(&NET_UPDATE) {
                net_list = storage::load_all_wifi_creds();
                idx = 0;
                // New credentials saved via the portal: exit forced-AP mode and
                // let the normal connect loop pick the best network.
                force_ap_mode = false;
                let conf = match net_list.first() {
                    Some(c) => net_config(ap_enabled, c),
                    None => Config::AccessPointStation(
                        StationConfig::default(),
                        AccessPointConfig::default()
                            .with_ssid(AP_SSID.try_into().unwrap()),
                    ),
                };
                println!("[wifi] applying updated station config");
                let _ = controller.set_config(&conf);
                SCAN_REQUEST.store(1, Ordering::Release); // refresh list for next page load
            }

            if take_flag(&SCAN_REQUEST) {
                let scan = scan_aps(&mut controller).await;
                #[allow(static_mut_refs)]
                unsafe {
                    SCAN_CACHE = scan;
                }
                SCAN_READY.store(1, Ordering::Release);
                println!("[wifi] scan done");
            }
        }

        // While forcibly re-provisioning, serve the SoftAP and wait for the user
        // to save credentials; don't try to auto-connect to a saved network that
        // would take the AP back down.
        if force_ap_mode {
            Timer::after(Duration::from_millis(200)).await;
            continue;
        }

        // 2) (Re)load the saved networks if we don't have them yet.
        if net_list.is_empty() {
            net_list = storage::load_all_wifi_creds();
        }
        if net_list.is_empty() {
            // No saved networks: provisioning mode, nothing to connect to.
            Timer::after(Duration::from_millis(100)).await;
            continue;
        }

        // 3) Try each saved network in turn until one connects.
        let n = net_list.len();
        let start = idx % n;
        let confident = 'outer: loop {
            let i = idx % n;
            let conf = net_config(ap_enabled, &net_list[i]);
            println!(
                "[wifi] trying `{}` ({}/{})",
                net_list[i].ssid, i + 1, n
            );
            let _ = controller.set_config(&conf);

            match embassy_futures::select::select(
                controller.connect_async(),
                Timer::after(CONNECT_TIMEOUT),
            )
            .await
            {
                embassy_futures::select::Either::First(Ok(_)) => {
                    idx = i;
                    break 'outer true;
                }
                embassy_futures::select::Either::First(Err(_)) | embassy_futures::select::Either::Second(_) => {
                    idx = (i + 1) % n;
                    if idx == start {
                        break 'outer false; // tried every network this pass
                    }
                }
            }
        };

        if !confident {
            // All networks failed this round — back off, then retry from the top.
            println!("[wifi] all {} network(s) failed, retrying in 10s", n);
            idx = 0;
            Timer::after(Duration::from_secs(10)).await;
            continue;
        }

        // 4) Connected to net_list[idx].
        println!("[wifi] STA connected to `{}`", net_list[idx].ssid);
        if ap_enabled {
            // We have a real network now: turn the SoftAP off once.
            println!("[wifi] turning SoftAP off");
            let _ = controller.set_config(&net_config(false, &net_list[idx]));
            ap_enabled = false;
            // set_config resets the radio and drops the link; give it a moment,
            // then the next loop reconnects without the AP.
            Timer::after(Duration::from_millis(500)).await;
        } else {
            // AP already off: hold the link until the network drops, then the
            // next attempt moves on to another saved network if needed.
            let _ = controller.wait_for_disconnect_async().await;
            idx = (idx + 1) % n;
        }
    }
}

/// Run one Wi-Fi scan, bounded by [`SCAN_TIMEOUT`].  Returns an empty list on
/// timeout/error (the radio may be busy serving the SoftAP, in which case no
/// scan is possible — the setup form still lets the user type an SSID).
async fn scan_aps(controller: &mut WifiController<'static>) -> ScanResult {
    let mut list: ScanResult = HeaplessVec::new();
    let config = ScanConfig::default();
    println!("[wifi] scan starting (timeout {}s)...", SCAN_TIMEOUT.as_secs() as u32);
    let ctrl = controller.scan_async(&config);
    match embassy_futures::select::select(ctrl, Timer::after(SCAN_TIMEOUT)).await {
        embassy_futures::select::Either::First(Ok(aps)) => {
            let n = aps.len();
            for ap in aps {
                if list.push(ap).is_err() {
                    break;
                }
            }
            println!("[wifi] scan found {} APs", n);
        }
        embassy_futures::select::Either::First(Err(e)) => {
            println!("[wifi] scan error: {e:?}");
        }
        embassy_futures::select::Either::Second(_) => {
            println!("[wifi] scan timed out ({}s)", SCAN_TIMEOUT.as_secs() as u32);
        }
    }
    list
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
    out[0..2].copy_from_slice(&req[0..2]);
    out[2] = 0x81;
    out[3] = 0x80;
    out[4..6].copy_from_slice(&[0x00, 0x01]); // QDCOUNT
    out[6..8].copy_from_slice(&[0x00, 0x01]); // ANCOUNT
    out[8..12].fill(0);

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
    out[12..12 + qname_len].copy_from_slice(&req[qstart..i]);
    let mut pos = 12 + qname_len;
    if i + 4 <= req.len() {
        out[pos..pos + 4].copy_from_slice(&req[i..i + 4]);
        pos += 4;
    }
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
// NTP time task — resolves `time.google.com`, does a UDP SNTP round-trip and
// stores the Unix time into [`crate::time`] so the clock UI can show the real
// wall-clock time. Re-syncs every [`NTP_REFRESH`] while the STA is up.
// ---------------------------------------------------------------------------

#[embassy_executor::task]
async fn ntp_task(stack: Stack<'static>) -> ! {
    use edge_nal::{UdpBind, UdpReceive, UdpSend};
    use edge_nal_embassy::{Udp, UdpBuffers};

    // DNS resolution through the STA's DHCP-provided DNS server.
    let dns = DnsSocket::new(stack);

    let buffers = UdpBuffers::<2, 512, 512, 2>::new();
    let udp = Udp::new(stack, &buffers);
    let mut bound = udp
        .bind(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0)))
        .await
        .expect("bind NTP udp");

    let mut pkt = [0u8; 48];
    let mut resp = [0u8; 48];

    loop {
        // Wait for the STA to actually be connected and have an IP (DHCP).
        // Block silently, but log once if we're stuck waiting a long time so a
        // network that never hands out an IP (or a broken DHCP) is visible.
        while !stack.is_config_up() {
            Timer::after(Duration::from_secs(10)).await;
            if !stack.is_config_up() {
                println!(
                    "[ntp] still waiting for STA IP (link_up={}, config_up={})",
                    stack.is_link_up(),
                    stack.is_config_up()
                );
            }
        }
        println!("[ntp] STA config up (link={})", stack.is_link_up());

        // Resolve the NTP host (fresh each round so DNS failures don't stick).
        // `DnsSocket::query()` blocks for as long as it takes with no built-in
        // timeout, so we race it against a timer to avoid wedging the task (and
        // silently never synchronising the clock).
        let mut servers: [Ipv4Addr; 3] = [
            Ipv4Addr::new(216, 239, 35, 0), // Google NTP anycast (pre-resolved fallback)
            NTP_FALLBACKS[0],
            NTP_FALLBACKS[1],
        ];
        let query = dns.query(NTP_HOST, embassy_net::dns::DnsQueryType::A);
        let resolved = embassy_futures::select::select(
            query,
            Timer::after(Duration::from_secs(5)),
        )
        .await;
        if let embassy_futures::select::Either::First(Ok(mut addrs)) = resolved {
            while let Some(a) = addrs.pop() {
                if let embassy_net::IpAddress::Ipv4(v4) = a {
                    servers[0] = Ipv4Addr::from(v4);
                    break;
                }
            }
            println!(
                "[ntp] trying {} and {} fallbacks",
                NTP_HOST,
                NTP_FALLBACKS.len()
            );
        } else {
            println!("[ntp] DNS for {NTP_HOST} timed out; using fallbacks");
        }

        // Build a simple SNTP client request (mode 3 = client) once.
        pkt.fill(0);
        pkt[0] = 0x1B; // LI=0, VN=3, mode=3

        // Try each server until one answers with a valid 48-byte response.
        let mut unix: Option<i64> = None;
        for server in servers {
            let ntp_peer = SocketAddr::V4(SocketAddrV4::new(server, NTP_PORT));
            if bound.send(ntp_peer, &pkt).await.is_err() {
                continue; // couldn't send — move on to the next server
            }
            // Wait (bounded, per server) for a valid NTP response.
            let deadline = embassy_time::Instant::now() + Duration::from_secs(3);
            while embassy_time::Instant::now() < deadline {
                match bound.receive(&mut resp).await {
                    Ok((len, _peer)) => {
                        if len >= 48 {
                            // Transmit timestamp is bytes 40..48 (secs since 1900).
                            let secs = u32::from_be_bytes([
                                resp[40], resp[41], resp[42], resp[43],
                            ]);
                            unix = Some((secs as u64 - NTP_UNIX_OFFSET) as i64);
                        }
                        break;
                    }
                    Err(_) => {
                        Timer::after(Duration::from_millis(50)).await;
                    }
                }
            }
            if unix.is_some() {
                println!("[ntp] synced from {} -> unix {}", server, unix.unwrap());
                break;
            }
        }

        match unix {
            Some(u) => {
                crate::time::set_ntp_time(u);
            }
            None => println!("[ntp] sync failed (no NTP response)"),
        }

        Timer::after(NTP_REFRESH).await;
    }
}

// ---------------------------------------------------------------------------
// HTTP provision server — captive-portal intercept + setup UI.
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
        // Log the request line (first line up to CRLF) for debugging.
        let first_line = core::str::from_utf8(
            req.split(|&b| b == b'\n').next().unwrap_or(b""),
        )
        .unwrap_or("?");
        println!("[wifi] HTTP {first_line} ({})", req.len());

        if is_post_path(req, b"/remove") {
            // Fetch API: remove a saved network by SSID (form-encoded `ssid=`).
            // After a successful removal, tell the driver to drop that network
            // and refresh its connection list.
            let mut ssid = heapless::String::<32>::new();
            let mut _pw = heapless::String::<64>::new();
            parse_form(body_slice(req).unwrap_or(&[]), &mut ssid, &mut _pw);
            let removed = storage::remove_wifi_creds(ssid.as_str());
            println!("[wifi] HTTP /remove `{}` -> removed={}", ssid, removed);
            // Ask the driver to reload the saved list and drop the removed
            // network (re-apply station/AP config as needed).
            NET_UPDATE.store(1, Ordering::Release);
            let _ = socket.write_all(HTTP_JSON).await;
            serve_remove_json(&mut socket, removed).await;
        } else if is_post {
            let mut ssid = heapless::String::<32>::new();
            let mut password = heapless::String::<64>::new();
            parse_form(body_slice(req).unwrap_or(&[]), &mut ssid, &mut password);
            println!(
                "[wifi] POST parsed ssid=`{}` pass_len={}",
                ssid,
                password.len()
            );

            if !ssid.is_empty() && (password.is_empty() || password.len() >= 8) {
                let creds = storage::WifiCreds { ssid, password };
                storage::save_wifi_creds(&creds);
                // Ask the driver to apply the new config and connect — no reboot.
                NET_UPDATE.store(1, Ordering::Release);
                println!("[wifi] saved `{}`, connecting...", creds.ssid);
                let _ = socket.write_all(RESP_SAVED.as_bytes()).await;
                let _ = socket.flush().await;
            } else {
                let _ = socket.write_all(RESP_BAD.as_bytes()).await;
                let _ = socket.flush().await;
            }
        } else if !is_local_host(req) {
            // Captive portal: redirect any other host to our setup page.
            let _ = socket.write_all(REDIRECT).await;
        } else if is_path(req, b"/scan") {
            // Fetch API: trigger a fresh scan, wait (bounded) for it, then
            // return the result as JSON.  The page's JS fills the list in
            // place — no full-page reload.
            println!("[wifi] HTTP /scan (fetch)");
            SCAN_REQUEST.store(1, Ordering::Release);
            let deadline = embassy_time::Instant::now() + Duration::from_secs(5);
            while SCAN_READY.load(Ordering::Acquire) == 0
                && embassy_time::Instant::now() < deadline
            {
                Timer::after(Duration::from_millis(50)).await;
            }
            let scan = read_scan_cache();
            let _ = socket.write_all(HTTP_JSON).await;
            serve_scan_json(&mut socket, &scan).await;
        } else if is_path(req, b"/saved") {
            // Fetch API: return the list of saved networks as JSON so the page
            // can render them (and let the user remove entries).
            println!("[wifi] HTTP /saved");
            let _ = socket.write_all(HTTP_JSON).await;
            serve_saved_json(&mut socket).await;
        } else {
            // Local /: serve the setup page (JS loads the network list via
            // fetch("/scan") itself — never blocks the page render).
            let _ = socket.write_all(HTTP_200).await;
            let _ = socket.write_all(PAGE_HTML.as_bytes()).await;
        }

        let _ = socket.flush().await;
        socket.close();
        Timer::after(Duration::from_millis(150)).await;
    }
}

/// Grab a copy of the latest scan result (if the driver published one).
fn read_scan_cache() -> Option<ScanResult> {
    if SCAN_READY.load(Ordering::Acquire) == 1 {
        #[allow(static_mut_refs)]
        unsafe {
            Some(core::ptr::read(&SCAN_CACHE as *const ScanResult))
        }
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// /scan JSON rendering — the browser's fetch() parses this to fill the list.
// ---------------------------------------------------------------------------

async fn serve_scan_json(socket: &mut TcpSocket<'_>, scan: &Option<ScanResult>) {
    use embedded_io_async::Write;

    let _ = socket.write_all(b"[ ").await;
    if let Some(list) = scan {
        for (i, ap) in list.iter().enumerate() {
            if i > 0 {
                let _ = socket.write_all(b",").await;
            }
            let locked = ap.auth_method != Some(AuthenticationMethod::None);
            let _ = socket.write_all(b"{\"s\":\"").await;
            let _ = socket.write_all(&escape_json(ap.ssid.as_str().as_bytes())).await;
            let _ = socket.write_all(b"\",\"r\":").await;
            let _ = socket.write_all(&write_i8(ap.signal_strength)).await;
            let _ = socket.write_all(b",\"l\":").await;
            let _ = socket.write_all(&write_u8(locked as u8)).await;
            let _ = socket.write_all(b"}").await;
        }
    }
    let _ = socket.write_all(b" ]").await;
    let _ = socket.flush().await;
}

/// Stream the list of saved networks as a JSON array of `{s:"ssid"}` objects.
/// The page's JS renders this into the "Saved networks" section.
async fn serve_saved_json(socket: &mut TcpSocket<'_>) {
    use embedded_io_async::Write;

    let saved = storage::load_all_wifi_creds();
    let _ = socket.write_all(b"[ ").await;
    for (i, creds) in saved.iter().enumerate() {
        if i > 0 {
            let _ = socket.write_all(b",").await;
        }
        let _ = socket.write_all(b"{\"s\":\"").await;
        let _ = socket
            .write_all(&escape_json(creds.ssid.as_str().as_bytes()))
            .await;
        let _ = socket.write_all(b"\"}").await;
    }
    let _ = socket.write_all(b" ]").await;
    let _ = socket.flush().await;
}

/// Stream `{ok: true|false}` as JSON for the `/remove` endpoint.
async fn serve_remove_json(socket: &mut TcpSocket<'_>, removed: bool) {
    use embedded_io_async::Write;

    let _ = socket.write_all(b"{\"ok\":").await;
    let _ = socket.write_all(if removed { b"true" } else { b"false" }).await;
    let _ = socket.write_all(b"}").await;
    let _ = socket.flush().await;
}

// ---------------------------------------------------------------------------
// Response constants
// ---------------------------------------------------------------------------

const HTTP_200: &[u8] =
    b"HTTP/1.0 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nConnection: close\r\n\r\n";

const HTTP_JSON: &[u8] = b"HTTP/1.0 200 OK\r\nContent-Type: application/json; charset=utf-8\r\n\
    Cache-Control: no-store\r\nConnection: close\r\n\r\n";

const REDIRECT: &[u8] = b"HTTP/1.0 302 Found\r\n\
    Location: http://192.168.4.1/\r\n\
    Content-Length: 0\r\n\
    Connection: close\r\n\
    \r\n";

const RESP_SAVED: &str = "HTTP/1.0 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nConnection: close\r\n\r\n\
    <!DOCTYPE html><html><body style='font-family:sans-serif;text-align:center;margin-top:20vh'>\
    <h2>Saved! Connecting...</h2>\
    <p>The device is connecting to Wi-Fi. You can leave this setup network.</p>\
    </body></html>";

const RESP_BAD: &str = "HTTP/1.0 400 Bad Request\r\nContent-Type: text/html; charset=utf-8\r\nConnection: close\r\n\r\n\
    <!DOCTYPE html><html><body style='font-family:sans-serif'>\
    <h2>Invalid input</h2><p>Select a network and enter a password (8+ characters for WPA2).</p>\
    <a href='/'>Go back</a></body></html>";

// The setup page.  It never blocks on a scan: a small script calls
// fetch("/scan") (async) and renders the network list into #list, so the
// page paints instantly and updates in place — no reload.
const PAGE_HTML: &str = "\
<!DOCTYPE html>
<html lang=\"en\">
<head>
<meta charset=\"utf-8\">
<meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">
<title>Dasai Mochi Shin - WiFi</title>
<style>
*{box-sizing:border-box;font-family:system-ui,sans-serif}
body{margin:0;background:#0f0f13;color:#eee;min-height:100vh}
.h{padding:18px 16px;border-bottom:1px solid #26262e}
.h h1{font-size:17px;margin:0;font-weight:600;display:inline-block}
.rf{float:right;color:#6aa9ff;font-size:12px;text-decoration:none;line-height:22px;border:0;background:none;cursor:pointer}
.ht{padding:12px 16px 4px;font-size:13px;color:#9a9aa5}
.n{display:flex;align-items:center;gap:12px;padding:14px 16px;border-bottom:1px solid #1c1c22;cursor:pointer}
.n.sel{background:#17251d}
.g{height:7px;border-radius:1px;background:#8fd19e;display:inline-block;margin-right:1px}
.nm{flex:1;font-size:14px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}
.lk{color:#b3b3bd;font-size:13px}
.saved{display:flex;align-items:center;gap:12px;padding:12px 16px;border-bottom:1px solid #1c1c22}
.saved .nm{font-size:14px}
.del{margin-left:auto;background:none;border:1px solid #3a2430;color:#e77;border-radius:8px;padding:6px 11px;font-size:12px;cursor:pointer}
.badge{font-size:11px;color:#6aa9ff;border:1px solid #2c3d57;border-radius:6px;padding:2px 6px}
.pw{padding:16px}
input[type=text],input[type=password]{display:block;width:100%;padding:13px;margin-bottom:10px;border-radius:10px;\
border:1px solid #2c2c36;background:#17171d;color:#eee;font-size:15px}
.bt{margin:0 16px;width:calc(100% - 32px);padding:14px;border:0;border-radius:12px;background:#2ea36b;color:#fff;font-size:16px;font-weight:600}
.em{padding:26px 16px;text-align:center;color:#777;font-size:14px}
#list{padding-top:4px}
</style>
</head>
<body>
<div class=\"h\"><h1>Wi-Fi</h1><button class=\"rf\" id=\"scanbtn\">Rescan</button></div>
<form method=\"POST\" action=\"/\">
<p class=\"ht\" id=\"status\">Scanning networks...</p>
<div id=\"list\"><div class=\"em\">Scanning...</div></div>
<div class=\"pw\">
<input type=\"text\" name=\"ssid\" id=\"manual\" placeholder=\"Wi-Fi name (SSID)\" autocomplete=\"off\">
<input type=\"password\" name=\"password\" placeholder=\"Wi-Fi password\" autocomplete=\"off\">
</div>
<button type=\"submit\" class=\"bt\">Connect</button>
</form>
<p class=\"ht\">Saved networks (tap X to remove)</p>
<div id=\"savedlist\"><div class=\"em\">Loading...</div></div>
<script>
function bars(r){var s='',n=4;
 if(r>=-50)n=4;else if(r>=-65)n=3;else if(r>=-75)n=2;else if(r>=-85)n=1;else n=0;
 for(var i=0;i<4;i++){s+='<span class=\"g\"'+(i<n?'':' style=\"opacity:.3\"')+'></span>';}return s;}
function pick(el){
 var man=document.getElementById('manual');man.value=el.dataset.s;
 var rows=document.getElementsByClassName('n');
 for(var i=0;i<rows.length;i++){rows[i].classList.remove('sel');}
 el.classList.add('sel');
}
function load(){
 var st=document.getElementById('status');st.textContent='Scanning networks...';
 document.getElementById('list').innerHTML='<div class=\"em\">Scanning...</div>';
 fetch('/scan',{cache:'no-store'}).then(function(r){return r.json()}).then(function(aps){
  var list=document.getElementById('list');list.innerHTML='';
  if(!aps.length){st.textContent='No networks found — type the SSID manually';
   list.innerHTML='<div class=\"em\">No networks found. Enter the SSID and password below.</div>';
   return;}
  st.textContent='Tap a network, then enter the password and Connect';
  aps.forEach(function(ap){
   var l=document.createElement('div');l.className='n';
   l.setAttribute('onclick','pick(this)');
   l.dataset.s=ap.s;
   var sp=document.createElement('span');sp.innerHTML=bars(ap.r);
   var nm=document.createElement('span');nm.className='nm';nm.textContent=ap.s;
   l.appendChild(sp);l.appendChild(nm);
   if(ap.l){var lk=document.createElement('span');lk.className='lk';lk.textContent='🔒';l.appendChild(lk);}
   list.appendChild(l);
  });
 }).catch(function(){st.textContent='Scan failed. Enter the SSID manually.';
  document.getElementById('list').innerHTML='<div class=\"em\">Scan failed, enter the SSID and password below.</div>';});
}
function loadSaved(){
 var el=document.getElementById('savedlist');
 fetch('/saved',{cache:'no-store'}).then(function(r){return r.json()}).then(function(nets){
  el.innerHTML='';
  if(!nets.length){el.innerHTML='<div class=\"em\">No saved networks yet.</div>';return;}
  var title='Saved networks (tap X to remove)';
  nets.forEach(function(n){
   var d=document.createElement('div');d.className='saved';
   var nm=document.createElement('span');nm.className='nm';nm.textContent=n.s;
   var bd=document.createElement('span');bd.className='badge';bd.textContent='saved';
   var x=document.createElement('button');x.className='del';x.textContent='✕';
   x.onclick=function(e){e.stopPropagation();removeSaved(n.s);};
   d.appendChild(nm);d.appendChild(bd);d.appendChild(x);el.appendChild(d);
  });
 }).catch(function(){el.innerHTML='<div class=\"em\">Could not load saved networks.</div>';});
}
function removeSaved(ssid){
 if(!confirm('Remove saved network \"'+ssid+'\"?')){return;}
 var body='ssid='+encodeURIComponent(ssid);
 fetch('/remove',{method:'POST',headers:{'Content-Type':'application/x-www-form-urlencoded'},body:body})
  .then(function(r){return r.json()}).then(function(res){
   document.getElementById('manual').value='';
   loadSaved();
  }).catch(function(){});
}
document.getElementById('scanbtn').onclick=function(e){e.preventDefault();load();};
load();
loadSaved();
</script>
</body>
</html>
";

// ---------------------------------------------------------------------------
// HTML / HTTP helpers
// ---------------------------------------------------------------------------

fn escape_json(s: &[u8]) -> heapless::Vec<u8, 224> {
    let mut out = heapless::Vec::<u8, 224>::new();
    for &b in s {
        match b {
            b'\\' => {
                let _ = out.push(b'\\');
                let _ = out.push(b'\\');
            }
            b'"' => {
                let _ = out.push(b'\\');
                let _ = out.push(b'"');
            }
            b'\n' => {
                let _ = out.push(b'\\');
                let _ = out.push(b'n');
            }
            b'\r' => {
                let _ = out.push(b'\\');
                let _ = out.push(b'r');
            }
            b'\t' => {
                let _ = out.push(b'\\');
                let _ = out.push(b't');
            }
            b if b < 0x20 => {
                let _ = out.push(b'\\');
                let _ = out.push(b'u');
                let _ = out.push(b'0');
                let _ = out.push(b'0');
                let _ = out.push(hex_char((b >> 4) & 0xF));
                let _ = out.push(hex_char(b & 0xF));
            }
            _ => {
                let _ = out.push(b);
            }
        }
    }
    out
}

fn hex_char(v: u8) -> u8 {
    match v {
        0..=9 => b'0' + v,
        _ => b'a' + (v - 10),
    }
}

/// Serialize an `i8` as a decimal string (e.g. -60 → `-60`).
fn write_i8(v: i8) -> heapless::Vec<u8, 4> {
    let mut out = heapless::Vec::<u8, 4>::new();
    if v < 0 {
        let _ = out.push(b'-');
        for b in write_u8(v.unsigned_abs()) {
            let _ = out.push(b);
        }
    } else {
        for b in write_u8(v as u8) {
            let _ = out.push(b);
        }
    }
    out
}

/// Serialize a `u8` as a decimal string.
fn write_u8(v: u8) -> heapless::Vec<u8, 3> {
    let mut out = heapless::Vec::<u8, 3>::new();
    let mut digs = [0u8; 3];
    let mut len = 0;
    let mut m = v;
    loop {
        digs[len] = b'0' + (m % 10);
        len += 1;
        m /= 10;
        if m == 0 {
            break;
        }
    }
    for i in (0..len).rev() {
        let _ = out.push(digs[i]);
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

/// Check whether this is a `GET <path> ...` request.
fn is_path(req: &[u8], path: &[u8]) -> bool {
    req.starts_with(b"GET ") && {
        let rest = &req[4..];
        rest.starts_with(path) && rest.get(path.len()) == Some(&b' ')
    }
}

/// Check whether this is a `POST <path> ...` request.
fn is_post_path(req: &[u8], path: &[u8]) -> bool {
    req.starts_with(b"POST ") && {
        let rest = &req[5..];
        rest.starts_with(path) && rest.get(path.len()) == Some(&b' ')
    }
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
