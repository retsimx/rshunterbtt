use anyhow::{anyhow, Result};

pub fn reboot_host() -> Result<()> {
    let ret = unsafe { libc::reboot(libc::RB_AUTOBOOT) };
    if ret != 0 {
        return Err(anyhow!(
            "libc::reboot failed: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(())
}
