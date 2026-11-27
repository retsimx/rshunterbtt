use anyhow::{anyhow, Result};
use std::os::unix::io::RawFd;
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

/// Process-lifetime diagnostic context. This monitor observes HCI events but
/// never sends controller commands or triggers connection, adapter, or host recovery.
pub struct IntervalGuard {
    pub device_address: String,
    pub interval_ms: u16,
}

impl IntervalGuard {
    pub fn new(device_address: String, interval_ms: u16) -> Self {
        Self {
            device_address,
            interval_ms,
        }
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
    if data.len() < HCI_MON_HDR_SIZE + len {
        return;
    }
    let payload = &data[HCI_MON_HDR_SIZE..HCI_MON_HDR_SIZE + len];
    if opcode == HCI_MON_EVENT_PKT {
        decode_hci_event(payload, guard);
    }
}

fn decode_hci_event(evt: &[u8], _guard: &mut IntervalGuard) {
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
            let status = evt[3];
            let event_handle = u16::from_le_bytes([evt[4], evt[5]]);
            let interval = u16::from_le_bytes([evt[6], evt[7]]);
            info!(
                "HCI connection update complete: handle {} interval {}ms status 0x{:02x}",
                event_handle,
                interval as u32 * 5 / 4,
                status
            );
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

/// Watch the HCI event stream for process-lifetime diagnostics. The monitor
/// logs disconnections, selected controller command statuses (including
/// `0x2005`), and connection-update events. It never sends HCI commands and
/// never triggers connection, adapter, or host recovery.
///
/// The socket is bound synchronously before returning, so the caller knows
/// whether monitoring is active. Returns `true` if the monitor is bound and
/// running.
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
                "HCI monitor active: diagnostic controller logging (target interval {}ms)",
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
    fn test_connection_update_complete_is_diagnostic_only() {
        // LE Meta (0x3e), subevent Connection Update Complete (0x03),
        // status 0, handle 64, interval 0x0320 (=1000ms).
        let evt = [
            0x3eu8, 0x0c, 0x03, 0x00, 0x40, 0x00, 0x20, 0x03, 0x00, 0x00, 0xd0, 0x07,
        ];
        decode_monitor_packet(&event_pkt(&evt), &mut guard());
    }

    #[test]
    fn test_fast_connection_update_is_logged_without_recovery_action() {
        let mut g = IntervalGuard::new("11:22:33:44:55:66".to_string(), 60);
        let evt = [
            0x3eu8, 0x0c, 0x03, 0x00, 0x40, 0x00, 0x30, 0x00, 0x00, 0x00, 0xd0, 0x07,
        ];
        decode_monitor_packet(&event_pkt(&evt), &mut g);
    }
}
