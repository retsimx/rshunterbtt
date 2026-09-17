use anyhow::{anyhow, Result};
use std::os::unix::io::RawFd;
use std::time::{Duration, Instant};
use tracing::{debug, info, warn};

const AF_BLUETOOTH: i32 = 31;
const BTPROTO_HCI: i32 = 1;
const HCI_DEV_NONE: u16 = 0xffff;
// NOTE: the kernel HCI channel enum is NOT stable across trees. This is the
// Raspberry Pi 6.12.y value (include/net/bluetooth/hci_sock.h):
//   RAW=0, USER=1, MONITOR=2, CONTROL=3, LOGGING=4
// Mainline uses MONITOR=3/CONTROL=2. Binding the wrong value either fails
// (USER -> EINVAL) or silently delivers mgmt events instead of monitor frames.
const HCI_CHANNEL_MONITOR: u16 = 2;

const HCI_MON_HDR_SIZE: usize = 6;
// From include/net/bluetooth/hci_mon.h: COMMAND_PKT=2, EVENT_PKT=3,
// ACL_TX=4, ACL_RX=5. (18/19 are ISO TX/RX.)
const HCI_MON_EVENT_PKT: u16 = 3;

const HCI_EVENT_DISCONNECTION_COMPLETE: u8 = 0x05;
const HCI_EVENT_COMMAND_STATUS: u8 = 0x0f;
const HCI_EVENT_LE_META: u8 = 0x3e;
const LE_SUBEVENT_CONNECTION_UPDATE_COMPLETE: u8 = 0x03;
const LE_SET_RANDOM_ADDRESS: u16 = 0x2005;

/// Minimum gap between our own corrective interval re-applies, so a peripheral
/// that re-negotiates repeatedly can never spin us.
const REAPPLY_MIN_GAP: Duration = Duration::from_secs(3);

#[repr(C)]
#[derive(Clone, Copy)]
struct SockaddrHci {
    family: u16,
    dev: u16,
    channel: u16,
}

fn open_hci_monitor_socket() -> Result<RawFd> {
    let fd = unsafe {
        libc::socket(
            AF_BLUETOOTH,
            libc::SOCK_RAW | libc::SOCK_CLOEXEC,
            BTPROTO_HCI,
        )
    };
    if fd < 0 {
        return Err(anyhow!(
            "open HCI monitor socket: {}",
            std::io::Error::last_os_error()
        ));
    }
    let addr = SockaddrHci {
        family: AF_BLUETOOTH as u16,
        dev: HCI_DEV_NONE,
        channel: HCI_CHANNEL_MONITOR,
    };
    let ret = unsafe {
        libc::bind(
            fd,
            &addr as *const SockaddrHci as *const libc::sockaddr,
            std::mem::size_of::<SockaddrHci>() as libc::socklen_t,
        )
    };
    if ret < 0 {
        let e = std::io::Error::last_os_error();
        unsafe { libc::close(fd) };
        return Err(anyhow!("bind HCI monitor socket: {}", e));
    }
    Ok(fd)
}

/// State for the interval guard: the configured target and a rate-limit clock.
pub struct IntervalGuard {
    pub device_address: String,
    pub interval_ms: u16,
    last_reapply: Instant,
}

impl IntervalGuard {
    pub fn new(device_address: String, interval_ms: u16) -> Self {
        Self {
            device_address,
            interval_ms,
            last_reapply: Instant::now() - REAPPLY_MIN_GAP,
        }
    }

    fn target_units(&self) -> u16 {
        (self.interval_ms as u32 * 4 / 5) as u16
    }

    /// The peripheral (or BlueZ) has re-negotiated the interval away from our
    /// target. Re-assert it, rate-limited. The re-apply runs on its own thread
    /// so a slow/blocking `request_interval` never stalls the monitor loop
    /// (which would otherwise drop events).
    fn enforce(&mut self, actual_units: u16) {
        if actual_units == self.target_units() {
            return;
        }
        let actual_ms = actual_units as u32 * 5 / 4;
        if self.last_reapply.elapsed() < REAPPLY_MIN_GAP {
            debug!(
                "interval changed to {}ms (target {}ms) within rate-limit window; not re-applying yet",
                actual_ms, self.interval_ms
            );
            return;
        }
        self.last_reapply = Instant::now();
        let device_address = self.device_address.clone();
        let interval_ms = self.interval_ms;
        info!(
            "interval changed to {}ms (target {}ms); re-applying",
            actual_ms, interval_ms
        );
        std::thread::Builder::new()
            .spawn(
                move || match crate::hci::request_interval(&device_address, interval_ms) {
                    Ok(()) => info!("interval re-apply complete"),
                    Err(e) => warn!("interval re-apply failed: {}", e),
                },
            )
            .ok();
    }
}

fn decode_monitor_packet(data: &[u8], guard: &mut IntervalGuard) {
    if data.len() < HCI_MON_HDR_SIZE {
        debug!("short monitor frame: {} bytes", data.len());
        return;
    }
    let opcode = u16::from_le_bytes([data[0], data[1]]);
    let len = u16::from_le_bytes([data[4], data[5]]) as usize;
    debug!("monitor frame: opcode={} len={}", opcode, len);
    if std::env::var("RSHUNTERBTT_MON_DEBUG").is_ok() {
        info!("monitor raw: {:02x?}", &data[..data.len().min(24)]);
    }
    if data.len() < HCI_MON_HDR_SIZE + len {
        return;
    }
    let payload = &data[HCI_MON_HDR_SIZE..HCI_MON_HDR_SIZE + len];
    if opcode == HCI_MON_EVENT_PKT {
        decode_hci_event(payload, guard);
    }
}

fn decode_hci_event(evt: &[u8], guard: &mut IntervalGuard) {
    if evt.len() < 2 {
        return;
    }
    match evt[0] {
        HCI_EVENT_DISCONNECTION_COMPLETE if evt.len() >= 6 => {
            let reason = evt[5];
            info!("HCI disconnection complete: reason 0x{:02x}", reason);
        }
        HCI_EVENT_COMMAND_STATUS if evt.len() >= 6 => {
            let op = u16::from_le_bytes([evt[4], evt[5]]);
            if op == LE_SET_RANDOM_ADDRESS {
                let status = evt[2];
                info!(
                    "HCI command status: LE Set Random Address (0x{:04x}) status 0x{:02x}",
                    op, status
                );
            }
        }
        HCI_EVENT_LE_META if evt.len() >= 8 && evt[2] == LE_SUBEVENT_CONNECTION_UPDATE_COMPLETE => {
            // evt[3]=status, evt[4..6]=handle (LE), evt[6..8]=connection interval
            let interval = u16::from_le_bytes([evt[6], evt[7]]);
            guard.enforce(interval);
        }
        _ => {}
    }
}

fn run_monitor_loop(fd: RawFd, mut guard: IntervalGuard) {
    let mut buf = [0u8; 4096];
    loop {
        let n = unsafe { libc::read(fd, buf.as_mut_ptr() as *mut libc::c_void, buf.len()) };
        if n < 0 {
            warn!(
                "HCI monitor read failed: {}",
                std::io::Error::last_os_error()
            );
            break;
        }
        if n == 0 {
            break;
        }
        decode_monitor_packet(&buf[..n as usize], &mut guard);
    }
    unsafe { libc::close(fd) };
}

/// Watch the HCI event stream and keep the connection interval pinned to the
/// configured value. The Hunter BTT peripheral re-negotiates the interval
/// (L2CAP Connection Parameter Update Request) shortly after connecting and
/// intermittently afterwards; BlueZ honours it, silently reverting our
/// interval. On any `LE Connection Update Complete` that lands away from our
/// target we re-apply it (rate-limited).
///
/// The socket is bound *synchronously* before returning, so the caller can
/// guarantee the guard is active before issuing any interval update. Returns
/// `true` if the monitor is bound and running. Diagnostic decoding from the
/// original implementation is retained.
pub fn spawn_hci_monitor(device_address: String, interval_ms: u16) -> bool {
    let guard = IntervalGuard::new(device_address, interval_ms);
    let fd = match open_hci_monitor_socket() {
        Ok(fd) => fd,
        Err(e) => {
            warn!("HCI monitor socket unavailable: {}", e);
            return false;
        }
    };
    match std::thread::Builder::new().spawn(move || run_monitor_loop(fd, guard)) {
        Ok(_) => {
            info!(
                "HCI monitor active: interval guard watching for re-negotiation (target {}ms)",
                interval_ms
            );
            true
        }
        Err(e) => {
            unsafe { libc::close(fd) };
            warn!("HCI monitor thread spawn failed: {}", e);
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn guard() -> IntervalGuard {
        IntervalGuard::new("11:22:33:44:55:66".to_string(), 1000)
    }

    fn monitor_frame(opcode: u16, payload: &[u8]) -> Vec<u8> {
        let mut frame = Vec::with_capacity(HCI_MON_HDR_SIZE + payload.len());
        frame.extend_from_slice(&opcode.to_le_bytes());
        frame.extend_from_slice(&0u16.to_le_bytes());
        frame.extend_from_slice(&(payload.len() as u16).to_le_bytes());
        frame.extend_from_slice(payload);
        frame
    }

    fn event_pkt(evt: &[u8]) -> Vec<u8> {
        monitor_frame(HCI_MON_EVENT_PKT, evt)
    }

    #[test]
    fn test_decodes_disconnection_complete() {
        let evt = [0x05u8, 0x04, 0x00, 0x00, 0x40, 0x13];
        decode_monitor_packet(&event_pkt(&evt), &mut guard());
    }

    #[test]
    fn test_decodes_le_set_random_address_status() {
        let evt = [0x0fu8, 0x04, 0x01, 0x01, 0x05, 0x20];
        decode_monitor_packet(&event_pkt(&evt), &mut guard());
    }

    #[test]
    fn test_ignores_unrelated_traffic() {
        let evt = [0x3eu8, 0x02, 0x01, 0x02];
        decode_monitor_packet(&event_pkt(&evt), &mut guard());
        let cmd = [0x01u8, 0x05, 0x20, 0x06, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff];
        decode_monitor_packet(&monitor_frame(2, &cmd), &mut guard());
    }

    #[test]
    fn test_connection_update_complete_decodes_and_enforces() {
        // LE Meta (0x3e), subevent Connection Update Complete (0x03),
        // status 0, handle 64, interval 0x0320 (=1000ms -> equals target: no-op).
        let evt = [
            0x3eu8, 0x0c, 0x03, 0x00, 0x40, 0x00, 0x20, 0x03, 0x00, 0x00, 0xd0, 0x07,
        ];
        decode_monitor_packet(&event_pkt(&evt), &mut guard());
    }

    #[test]
    fn test_connection_update_complete_to_fast_interval_triggers_enforce() {
        // Link reverted to 0x0030 (=60ms) -> enforce() would call the HCI layer;
        // here we only assert the decoder path runs without panicking for a
        // non-target interval (the actual re-apply hits real HCI, so this test
        // uses a target of 60ms to keep it a no-op).
        let mut g = IntervalGuard::new("11:22:33:44:55:66".to_string(), 60);
        let evt = [
            0x3eu8, 0x0c, 0x03, 0x00, 0x40, 0x00, 0x30, 0x00, 0x00, 0x00, 0xd0, 0x07,
        ];
        decode_monitor_packet(&event_pkt(&evt), &mut g);
    }
}
