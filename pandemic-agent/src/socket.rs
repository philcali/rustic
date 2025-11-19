use anyhow::Result;
use std::ffi::CString;
use tracing::{info, warn};

use crate::Args;

#[cfg(target_os = "linux")]
use std::os::unix::fs::PermissionsExt;

#[cfg(not(target_os = "linux"))]
pub trait PermissionsExt {
    fn from_mode(_mode: u32) -> std::fs::Permissions {
        std::fs::Permissions::from(
            std::fs::File::open("/dev/null")
                .unwrap()
                .metadata()
                .unwrap()
                .permissions(),
        )
    }
}

pub fn setup_socket_permissions(args: &Args) -> Result<()> {
    // Set socket permissions
    std::fs::set_permissions(&args.socket_path, std::fs::Permissions::from_mode(0o660))?;

    // Change ownership to pandemic user so REST module can access it
    if let Err(e) = set_socket_ownership(args) {
        warn!("Failed to set socket ownership: {}", e);
    }

    Ok(())
}

fn set_socket_ownership(args: &Args) -> Result<()> {
    let user_cstr = CString::new(args.user.as_bytes())?;
    let group_cstr = CString::new(args.group.as_bytes())?;
    let path_cstr = CString::new(args.socket_path.to_string_lossy().as_bytes())?;

    // Get user info
    let passwd = unsafe { libc::getpwnam(user_cstr.as_ptr()) };
    if passwd.is_null() {
        return Err(anyhow::anyhow!("User '{}' not found", args.user));
    }
    let uid = unsafe { (*passwd).pw_uid };

    // Get group info
    let group = unsafe { libc::getgrnam(group_cstr.as_ptr()) };
    if group.is_null() {
        return Err(anyhow::anyhow!("Group '{}' not found", args.group));
    }
    let gid = unsafe { (*group).gr_gid };

    // Change ownership
    let result = unsafe { libc::chown(path_cstr.as_ptr(), uid, gid) };
    if result != 0 {
        return Err(anyhow::anyhow!(
            "chown failed: {}",
            std::io::Error::last_os_error()
        ));
    }

    info!("Socket ownership changed to {}:{}", args.user, args.group);
    Ok(())
}
