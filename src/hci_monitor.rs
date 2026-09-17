use anyhow::{anyhow, Result};
use std::os::unix::io::RawFd;
use tracing::{info, warn};

const AF_BLUETOOTH: i32 = 31;
const BTPROTO_HCI: i32 = 1;
const HCI_DEV_NONE: u16 = 0xffff;
const HCI_CHANNEL_MONITOR: u16 = 1;

const HCI_MON_HDR_SIZE: usize = 6;
const HCI_MON_EVENT_PKT: u16 = 3;

const HCI_EVENT_DISCONNECTION_COMPLETE: u8 = 0x05;
const HCI_EVENT_COMMAND_STATUS: u8 = 0x0f;
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

fn decode_monitor_packet(data: &[u8]) {
    if data.len() < HCI_MON_HDR_SIZE {
        return;
    }
    let opcode = u16::from_le_bytes([data[0], data[1]]);
    let len = u16::from_le_bytes([data[4], data[5]]) as usize;
    if data.len() < HCI_MON_HDR_SIZE + len {
        return;
    }
    let payload = &data[HCI_MON_HDR_SIZE..HCI_MON_HDR_SIZE + len];
    if opcode == HCI_MON_EVENT_PKT {
        decode_hci_event(payload);
    }
}

fn decode_hci_event(evt: &[u8]) {
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
        _ => {}
    }
}

fn run_monitor_loop(fd: RawFd) {
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
        decode_monitor_packet(&buf[..n as usize]);
    }
    unsafe { libc::close(fd) };
}

pub fn spawn_hci_monitor() {
    let fd = match open_hci_monitor_socket() {
        Ok(fd) => fd,
        Err(e) => {
            warn!("HCI monitor socket unavailable (diagnostic only): {}", e);
            return;
        }
    };
    match std::thread::Builder::new().spawn(move || run_monitor_loop(fd)) {
        Ok(_) => {}
        Err(e) => {
            unsafe { libc::close(fd) };
            warn!("HCI monitor thread spawn failed: {}", e);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        decode_monitor_packet(&event_pkt(&evt));
    }

    #[test]
    fn test_decodes_le_set_random_address_status() {
        let evt = [0x0fu8, 0x04, 0x01, 0x01, 0x05, 0x20];
        decode_monitor_packet(&event_pkt(&evt));
    }

    #[test]
    fn test_ignores_unrelated_traffic() {
        let evt = [0x3eu8, 0x02, 0x01, 0x02];
        decode_monitor_packet(&event_pkt(&evt));
        let cmd = [0x01u8, 0x05, 0x20, 0x06, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff];
        decode_monitor_packet(&monitor_frame(2, &cmd));
    }
}
