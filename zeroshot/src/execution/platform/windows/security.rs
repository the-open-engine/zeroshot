//! Explicit current-user/SYSTEM ACLs for local state and named pipes.

use std::io;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{HANDLE, LocalFree};
use windows_sys::Win32::Security::{
    ACCESS_ALLOWED_ACE, ACE_HEADER, ACL, DACL_SECURITY_INFORMATION, GetAce,
    GetSecurityDescriptorDacl, GetTokenInformation, OWNER_SECURITY_INFORMATION,
    PROTECTED_DACL_SECURITY_INFORMATION, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER, TokenUser,
};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, GetSecurityInfo,
    SDDL_REVISION_1, SE_FILE_OBJECT, SetSecurityInfo,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

use super::check;

pub(crate) struct PrivateSecurity(*mut std::ffi::c_void);

impl Drop for PrivateSecurity {
    fn drop(&mut self) {
        unsafe {
            LocalFree(self.0);
        }
    }
}

impl PrivateSecurity {
    pub(crate) fn new() -> io::Result<Self> {
        let sid = current_sid()?;
        let sddl: Vec<_> = format!("O:{sid}D:P(A;OICI;FA;;;{sid})(A;OICI;FA;;;SY)")
            .encode_utf16()
            .chain(Some(0))
            .collect();
        let mut descriptor = null_mut();
        check(unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                SDDL_REVISION_1,
                &mut descriptor,
                null_mut(),
            )
        })?;
        Ok(Self(descriptor))
    }

    pub(crate) fn attributes(&self) -> SECURITY_ATTRIBUTES {
        SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: self.0,
            bInheritHandle: 0,
        }
    }

    fn dacl(&self) -> io::Result<*mut ACL> {
        let mut acl = null_mut();
        let mut present = 0;
        let mut defaulted = 0;
        check(unsafe {
            GetSecurityDescriptorDacl(self.0, &mut present, &mut acl, &mut defaulted)
        })?;
        if present == 0 || acl.is_null() {
            return Err(denied());
        }
        Ok(acl)
    }
}

fn current_sid() -> io::Result<String> {
    let mut token = null_mut();
    check(unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) })?;
    let token = unsafe { OwnedHandle::from_raw_handle(token) };
    let mut size = 0;
    unsafe {
        GetTokenInformation(token.as_raw_handle(), TokenUser, null_mut(), 0, &mut size);
    }
    if size == 0 {
        return Err(io::Error::last_os_error());
    }
    let mut storage = vec![0usize; (size as usize).div_ceil(std::mem::size_of::<usize>())];
    check(unsafe {
        GetTokenInformation(
            token.as_raw_handle(),
            TokenUser,
            storage.as_mut_ptr().cast(),
            size,
            &mut size,
        )
    })?;
    let user = unsafe { &*storage.as_ptr().cast::<TOKEN_USER>() };
    sid_string(user.User.Sid)
}

fn sid_string(sid: *mut std::ffi::c_void) -> io::Result<String> {
    let mut text = null_mut();
    check(unsafe { ConvertSidToStringSidW(sid, &mut text) })?;
    let allocated = PrivateSecurity(text.cast());
    let mut length = 0;
    unsafe {
        while *text.add(length) != 0 {
            length += 1;
        }
    }
    let value = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(text, length) });
    drop(allocated);
    Ok(value)
}

fn descriptor(handle: HANDLE) -> io::Result<(PrivateSecurity, String)> {
    let mut owner = null_mut();
    let mut security = null_mut();
    let result = unsafe {
        GetSecurityInfo(
            handle,
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &mut owner,
            null_mut(),
            null_mut(),
            null_mut(),
            &mut security,
        )
    };
    if result != 0 {
        return Err(io::Error::from_raw_os_error(result as i32));
    }
    let security = PrivateSecurity(security);
    let sid = current_sid()?;
    if owner.is_null() || sid_string(owner)? != sid {
        return Err(denied());
    }
    Ok((security, sid))
}

pub(crate) fn validate(handle: HANDLE) -> io::Result<()> {
    let (descriptor, sid) = descriptor(handle)?;
    let acl = descriptor.dacl()?;
    for index in 0..unsafe { (*acl).AceCount } {
        let mut ace = null_mut();
        check(unsafe { GetAce(acl, u32::from(index), &mut ace) })?;
        let header = unsafe { &*ace.cast::<ACE_HEADER>() };
        // Inspect the header before interpreting the variable-sized ACE payload.
        if header.AceType != 0
            || usize::from(header.AceSize) < std::mem::size_of::<ACCESS_ALLOWED_ACE>()
        {
            return Err(denied());
        }
        let allowed = unsafe { &*ace.cast::<ACCESS_ALLOWED_ACE>() };
        let trustee = sid_string(std::ptr::addr_of!(allowed.SidStart).cast_mut().cast())?;
        if trustee != sid && trustee != "S-1-5-18" {
            return Err(denied());
        }
    }
    Ok(())
}

pub(crate) fn protect(handle: HANDLE) -> io::Result<()> {
    descriptor(handle)?;
    let security = PrivateSecurity::new()?;
    let result = unsafe {
        SetSecurityInfo(
            handle,
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            null_mut(),
            null_mut(),
            security.dacl()?,
            null(),
        )
    };
    if result != 0 {
        return Err(io::Error::from_raw_os_error(result as i32));
    }
    validate(handle)
}

fn denied() -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        "object is not private to this user",
    )
}
