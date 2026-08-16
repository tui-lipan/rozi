use std::io;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;

use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{
    CreateDirectoryW, CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
    FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING, RemoveDirectoryW,
};
use windows_sys::Win32::System::IO::DeviceIoControl;

const IO_REPARSE_TAG_MOUNT_POINT: u32 = 0xA000_0003;
const FSCTL_SET_REPARSE_POINT: u32 = 0x0009_00A4;
const GENERIC_WRITE: u32 = 0x4000_0000;
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;

pub(super) fn is_junction(meta: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
        && meta.is_dir()
        && !meta.file_type().is_symlink()
}

/// Directory junction (`IO_REPARSE_TAG_MOUNT_POINT`). Does not require Administrator or Developer
/// Mode, unlike a directory symlink.
pub(super) fn create_junction(link: &Path, target: &Path) -> io::Result<()> {
    let target = std::fs::canonicalize(target)?;
    let nt_target = nt_path(&target);
    let nt_wide: Vec<u16> = nt_target.encode_utf16().collect();
    let buffer = mount_point_buffer(&nt_wide);

    let link_wide = wide(link);
    let created = unsafe { CreateDirectoryW(link_wide.as_ptr(), std::ptr::null()) };
    if created == 0 {
        return Err(io::Error::last_os_error());
    }

    let handle = unsafe {
        CreateFileW(
            link_wide.as_ptr(),
            GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            std::ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        let error = io::Error::last_os_error();
        unsafe {
            RemoveDirectoryW(link_wide.as_ptr());
        }
        return Err(error);
    }

    let mut bytes = 0u32;
    let ok = unsafe {
        DeviceIoControl(
            handle,
            FSCTL_SET_REPARSE_POINT,
            buffer.as_ptr() as *const std::ffi::c_void,
            buffer.len() as u32,
            std::ptr::null_mut(),
            0,
            &mut bytes,
            std::ptr::null_mut(),
        )
    };
    unsafe {
        CloseHandle(handle);
    }
    if ok == 0 {
        let error = io::Error::last_os_error();
        unsafe {
            RemoveDirectoryW(link_wide.as_ptr());
        }
        return Err(error);
    }
    Ok(())
}

fn wide(path: &Path) -> Vec<u16> {
    path.as_os_str().encode_wide().chain(Some(0)).collect()
}

fn nt_path(path: &Path) -> String {
    let displayed = path.to_string_lossy();
    let stripped = displayed
        .strip_prefix(r"\\?\UNC\")
        .map(|rest| format!(r"\??\UNC\{rest}"))
        .or_else(|| {
            displayed
                .strip_prefix(r"\\?\")
                .map(|rest| format!(r"\??\{rest}"))
        })
        .unwrap_or_else(|| format!(r"\??\{displayed}"));
    stripped
}

fn mount_point_buffer(nt_target: &[u16]) -> Vec<u8> {
    let subst_bytes = nt_target.len() * 2;
    let path_bytes = subst_bytes + 2;
    let data_len = 8 + path_bytes;
    let mut buf = vec![0u8; 8 + data_len];
    buf[0..4].copy_from_slice(&IO_REPARSE_TAG_MOUNT_POINT.to_le_bytes());
    buf[4..6].copy_from_slice(&(data_len as u16).to_le_bytes());
    // SubstituteNameOffset = 0 (already zeroed).
    buf[10..12].copy_from_slice(&(subst_bytes as u16).to_le_bytes());
    buf[12..14].copy_from_slice(&(path_bytes as u16).to_le_bytes());
    // PrintNameLength = 0 (already zeroed); PrintName sits after the substitute NUL.
    let path_start = 16;
    for (index, unit) in nt_target.iter().enumerate() {
        let offset = path_start + index * 2;
        buf[offset..offset + 2].copy_from_slice(&unit.to_le_bytes());
    }
    buf
}

#[cfg(test)]
mod tests {
    use super::nt_path;
    use std::path::Path;

    #[test]
    fn nt_path_rewrites_verbatim_and_unc_prefixes() {
        assert_eq!(
            nt_path(Path::new(r"\\?\C:\Users\you\.agents\skills\rozi")),
            r"\??\C:\Users\you\.agents\skills\rozi"
        );
        assert_eq!(
            nt_path(Path::new(r"\\?\UNC\server\share\rozi")),
            r"\??\UNC\server\share\rozi"
        );
        assert_eq!(
            nt_path(Path::new(r"C:\Users\you\.agents\skills\rozi")),
            r"\??\C:\Users\you\.agents\skills\rozi"
        );
    }
}
