use anyhow::{anyhow, Result};
use std::os::unix::io::RawFd;
use std::time::{Duration, Instant};
use tracing::{debug, info};

const AF_BLUETOOTH: i32 = 31;
const BTPROTO_HCI: i32 = 1;
const HCI_DEV: u16 = 0;
const SOL_HCI: i32 = 0;
const HCI_FILTER: i32 = 2;
const HCI_EVENT_PKT: u32 = 4;

const HCI_EVENT_COMMAND_COMPLETE: u8 = 0x0e;
const HCI_EVENT_COMMAND_STATUS: u8 = 0x0f;
const HCI_EVENT_LE_META: u8 = 0x3e;
const LE_SUBEVENT_CONNECTION_UPDATE_COMPLETE: u8 = 0x03;

const HCI_LE_CONN_UPDATE: u16 = (0x08 << 10) | 0x0013;
#[cfg(target_pointer_width = "64")]
const HCIGETCONNLIST: libc::c_ulong = 0x800448d4;
#[cfg(target_pointer_width = "32")]
const HCIGETCONNLIST: libc::c_int = 0x800448d4u32 as libc::c_int;

const SUPERVISION_TIMEOUT_MS: u16 = 20000;
const MAX_INTERVAL_RETRIES: usize = 3;
const INTERVAL_RETRY_DELAY: Duration = Duration::from_secs(2);

fn with_retry<T>(
    attempts: usize,
    delay: Duration,
    mut attempt: impl FnMut() -> Result<T>,
) -> Result<T> {
    let mut last_err = None;
    for i in 0..attempts {
        match attempt() {
            Ok(v) => return Ok(v),
            Err(e) => {
                if i + 1 < attempts {
                    debug!(
                        "LE conn update attempt {}/{} failed ({}); retrying in {:?}",
                        i + 1,
                        attempts,
                        e,
                        delay
                    );
                    std::thread::sleep(delay);
                }
                last_err = Some(e);
            }
        }
    }
    Err(last_err.unwrap_or_else(|| anyhow!("no retry attempts configured")))
}

fn interval_units(ms: u16) -> u16 {
    (ms as u32 * 4 / 5) as u16
}

fn timeout_units(ms: u16) -> u16 {
    ms / 10
}

#[repr(C)]
#[derive(Clone, Copy)]
struct HciConnInfo {
    handle: u16,
    bdaddr: [u8; 6],
    type_: u8,
    out: u8,
    state: u16,
    link_mode: u32,
}

#[repr(C)]
struct HciConnListReq {
    dev_id: u16,
    conn_num: u16,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct SockaddrHci {
    family: u16,
    dev: u16,
    channel: u16,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct HciFilter {
    type_mask: u32,
    event_mask: [u32; 2],
    opcode: u16,
}

fn open_hci_socket() -> Result<RawFd> {
    let fd = unsafe {
        libc::socket(
            AF_BLUETOOTH,
            libc::SOCK_RAW | libc::SOCK_CLOEXEC,
            BTPROTO_HCI,
        )
    };
    if fd < 0 {
        return Err(anyhow!(
            "open HCI socket: {}",
            std::io::Error::last_os_error()
        ));
    }
    let addr = SockaddrHci {
        family: AF_BLUETOOTH as u16,
        dev: HCI_DEV,
        channel: 0,
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
        return Err(anyhow!("bind HCI socket: {}", e));
    }
    let flt = HciFilter {
        type_mask: 1 << HCI_EVENT_PKT,
        event_mask: [0xffffffff, 0xffffffff],
        opcode: 0,
    };
    let sret = unsafe {
        libc::setsockopt(
            fd,
            SOL_HCI,
            HCI_FILTER,
            &flt as *const HciFilter as *const libc::c_void,
            std::mem::size_of::<HciFilter>() as libc::socklen_t,
        )
    };
    if sret < 0 {
        let e = std::io::Error::last_os_error();
        unsafe { libc::close(fd) };
        return Err(anyhow!("set HCI filter: {}", e));
    }
    Ok(fd)
}

fn send_hci_command(fd: RawFd, opcode: u16, params: &[u8]) -> Result<()> {
    let mut buf = Vec::with_capacity(4 + params.len());
    buf.push(0x01); // HCI_COMMAND_PKT
    buf.extend_from_slice(&opcode.to_le_bytes());
    buf.push(params.len() as u8);
    buf.extend_from_slice(params);
    let n = unsafe { libc::write(fd, buf.as_ptr() as *const libc::c_void, buf.len()) };
    if n < 0 {
        return Err(anyhow!(
            "write HCI command: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(())
}

fn read_hci_event(fd: RawFd, timeout: Duration) -> Result<Vec<u8>> {
    let mut pfd = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };
    let ret = unsafe { libc::poll(&mut pfd, 1, timeout.as_millis() as i32) };
    if ret < 0 {
        return Err(anyhow!(
            "poll HCI socket: {}",
            std::io::Error::last_os_error()
        ));
    }
    if ret == 0 {
        return Err(anyhow!("timed out waiting for HCI event"));
    }
    let mut buf = [0u8; 1024];
    let n = unsafe { libc::read(fd, buf.as_mut_ptr() as *mut libc::c_void, buf.len()) };
    if n < 0 {
        return Err(anyhow!(
            "read HCI event: {}",
            std::io::Error::last_os_error()
        ));
    }
    // Raw HCI socket frames include a leading packet-type byte (0x04 = event).
    let data = &buf[..n as usize];
    if data.len() < 2 || data[0] != 0x04 {
        return Err(anyhow!(
            "unexpected HCI frame type: 0x{:02x}",
            data.first().copied().unwrap_or(0)
        ));
    }
    Ok(data[1..].to_vec())
}

fn find_connection_handle(fd: RawFd, address: &str) -> Result<u16> {
    const MAX_CONN: usize = 16;
    let req_size = std::mem::size_of::<HciConnListReq>();
    let info_size = std::mem::size_of::<HciConnInfo>();
    let mut buf = vec![0u8; req_size + MAX_CONN * info_size];
    let req = buf.as_mut_ptr() as *mut HciConnListReq;
    unsafe {
        (*req).dev_id = HCI_DEV;
        (*req).conn_num = MAX_CONN as u16;
    }
    let ret = unsafe { libc::ioctl(fd, HCIGETCONNLIST, buf.as_mut_ptr() as *mut libc::c_void) };
    if ret < 0 {
        return Err(anyhow!(
            "HCIGETCONNLIST ioctl: {}",
            std::io::Error::last_os_error()
        ));
    }
    let conn_num = unsafe { (*req).conn_num } as usize;
    let target = address.replace(':', "").to_lowercase();
    let info_ptr = unsafe { buf.as_ptr().add(req_size) as *const HciConnInfo };
    for i in 0..conn_num {
        let ci = unsafe { &*info_ptr.add(i) };
        let bdaddr_str = ci
            .bdaddr
            .iter()
            .rev()
            .map(|b| format!("{:02x}", b))
            .collect::<Vec<_>>()
            .join("");
        if bdaddr_str == target {
            return Ok(ci.handle);
        }
    }
    Err(anyhow!("device {} not found in connection list", address))
}

fn request_connection_interval(fd: RawFd, handle: u16, interval_ms: u16) -> Result<u16> {
    with_retry(MAX_INTERVAL_RETRIES, INTERVAL_RETRY_DELAY, || {
        let min = interval_units(interval_ms);
        let max = min;
        let timeout = timeout_units(SUPERVISION_TIMEOUT_MS);
        let mut params = Vec::with_capacity(14);
        params.extend_from_slice(&handle.to_le_bytes());
        params.extend_from_slice(&min.to_le_bytes());
        params.extend_from_slice(&max.to_le_bytes());
        params.extend_from_slice(&0u16.to_le_bytes());
        params.extend_from_slice(&timeout.to_le_bytes());
        params.extend_from_slice(&1u16.to_le_bytes());
        params.extend_from_slice(&1u16.to_le_bytes());
        send_hci_command(fd, HCI_LE_CONN_UPDATE, &params)?;

        let deadline = Instant::now() + Duration::from_secs(15);
        while Instant::now() < deadline {
            let evt = match read_hci_event(fd, Duration::from_secs(2)) {
                Ok(e) => e,
                Err(_) => continue,
            };
            if evt.len() < 2 {
                continue;
            }
            match evt[0] {
                HCI_EVENT_COMMAND_COMPLETE if evt.len() >= 6 => {
                    let evt_op = evt[3] as u16 | ((evt[4] as u16) << 8);
                    if evt_op == HCI_LE_CONN_UPDATE && evt[5] != 0 {
                        return Err(anyhow!(
                            "LE conn update command complete status: 0x{:02x}",
                            evt[5]
                        ));
                    }
                }
                HCI_EVENT_COMMAND_STATUS if evt.len() >= 6 => {
                    let evt_op = evt[4] as u16 | ((evt[5] as u16) << 8);
                    if evt_op == HCI_LE_CONN_UPDATE && evt[2] != 0 {
                        return Err(anyhow!("LE conn update command status: 0x{:02x}", evt[2]));
                    }
                }
                HCI_EVENT_LE_META
                    if evt.len() >= 8 && evt[2] == LE_SUBEVENT_CONNECTION_UPDATE_COMPLETE =>
                {
                    let status = evt[3];
                    let evt_handle = u16::from_le_bytes([evt[4], evt[5]]);
                    let interval = u16::from_le_bytes([evt[6], evt[7]]);
                    if evt_handle != handle {
                        continue;
                    }
                    if status != 0 {
                        return Err(anyhow!(
                            "device rejected connection update: status 0x{:02x}",
                            status
                        ));
                    }
                    return Ok(interval);
                }
                _ => {}
            }
        }
        Err(anyhow!("timed out waiting for connection update result"))
    })
}

pub fn request_interval(device_address: &str, interval_ms: u16) -> Result<()> {
    let fd = open_hci_socket()?;
    let result = (|| {
        let handle = find_connection_handle(fd, device_address)?;
        debug!("connection handle for {}: {}", device_address, handle);
        let negotiated = request_connection_interval(fd, handle, interval_ms)?;
        let negotiated_ms = negotiated as u32 * 5 / 4;
        info!(
            "connection interval updated: requested {}ms, negotiated {}ms",
            interval_ms, negotiated_ms
        );
        Ok(())
    })();
    unsafe { libc::close(fd) };
    result
}

/// Resolve the current HCI connection handle for `device_address`, or an error
/// when the device is not currently connected. Used by the interval guard to
/// ignore `LE Connection Update Complete` events belonging to other links.
pub fn resolve_connection_handle(device_address: &str) -> Result<u16> {
    let fd = open_hci_socket()?;
    let result = find_connection_handle(fd, device_address);
    unsafe { libc::close(fd) };
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_interval_units() {
        assert_eq!(interval_units(4000), 3200);
        assert_eq!(interval_units(2000), 1600);
        assert_eq!(interval_units(1000), 800);
        assert_eq!(interval_units(100), 80);
    }

    #[test]
    fn test_timeout_units() {
        assert_eq!(timeout_units(20000), 2000);
        assert_eq!(timeout_units(10000), 1000);
        assert_eq!(timeout_units(3000), 300);
    }

    #[test]
    fn test_with_retry_succeeds_first_attempt() {
        let mut calls = 0;
        let result = with_retry(3, Duration::from_millis(1), || {
            calls += 1;
            Ok(42u16)
        });
        assert_eq!(result.unwrap(), 42);
        assert_eq!(calls, 1);
    }

    #[test]
    fn test_with_retry_succeeds_after_rejections() {
        let mut calls = 0;
        let result = with_retry(3, Duration::from_millis(1), || {
            calls += 1;
            if calls < 3 {
                Err(anyhow!("rejected"))
            } else {
                Ok(42u16)
            }
        });
        assert_eq!(result.unwrap(), 42);
        assert_eq!(calls, 3);
    }

    #[test]
    fn test_with_retry_gives_up_after_cap() {
        let mut calls = 0;
        let result = with_retry(3, Duration::from_millis(1), || {
            calls += 1;
            Err::<u16, _>(anyhow!("rejected"))
        });
        assert!(result.is_err());
        assert_eq!(calls, 3);
    }
}
