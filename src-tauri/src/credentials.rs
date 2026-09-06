//! Credential storage (T-205/T-206): API keys live behind a `CredentialStore`
//! abstraction — on Windows, the OS Credential Manager (advapi32's
//! CredRead/CredWrite, *not* a home-grown crypto scheme) — and the API
//! library keeps only references. Nothing here ever logs a secret.
//!
//! Resolution order everywhere a key is consumed: the caller's one-time
//! input → the credential store → the environment variable the provider
//! names. Old `api.json` files that carry plaintext keys are migrated into
//! the store on load and the file is rewritten without them.

/* ------------------------------- trait --------------------------------- */

pub trait CredentialStore: Send + Sync {
    fn get(&self, id: &str) -> Result<Option<String>, String>;
    fn set(&self, id: &str, secret: &str) -> Result<(), String>;
    fn delete(&self, id: &str) -> Result<(), String>;
}

/// The credential entry for one provider id.
///
/// The namespace is deliberately **global** — one entry per provider id,
/// shared by every data root. Two roots that both configure a provider named
/// `deepseek` therefore read and write the same key. That is a product
/// decision, not an accident: the id is the user-visible API endpoint
/// identity, and per-root copies would leak the same secret into extra
/// entries the user can neither see nor clean. A root migration moves the
/// configuration files but never re-keys credentials; the ids survive, so
/// entries stay reachable across relocations by construction.
pub fn provider_credential_id(provider_id: &str) -> String {
    // `PHL:` namespaces our entries inside the user's credential manager;
    // provider ids are already validated (`valid_provider_name`).
    format!("PHL:provider:{provider_id}")
}

/// The concrete store managed as Tauri state. Non-Windows builds get a
/// store that reports unsupported — the platform gates (T-301/T-302) carry
/// their own backends later.
pub struct Creds {
    inner: Box<dyn CredentialStore>,
}

impl Creds {
    pub fn platform_default() -> Self {
        Self {
            inner: Box::new(PlatformStore),
        }
    }
}

impl CredentialStore for Creds {
    fn get(&self, id: &str) -> Result<Option<String>, String> {
        self.inner.get(id)
    }
    fn set(&self, id: &str, secret: &str) -> Result<(), String> {
        self.inner.set(id, secret)
    }
    fn delete(&self, id: &str) -> Result<(), String> {
        self.inner.delete(id)
    }
}

struct PlatformStore;

#[cfg(windows)]
impl CredentialStore for PlatformStore {
    fn get(&self, id: &str) -> Result<Option<String>, String> {
        win::read(&provider_credential_id(id))
    }
    fn set(&self, id: &str, secret: &str) -> Result<(), String> {
        win::write(&provider_credential_id(id), secret)
    }
    fn delete(&self, id: &str) -> Result<(), String> {
        win::delete(&provider_credential_id(id))
    }
}

#[cfg(not(windows))]
impl CredentialStore for PlatformStore {
    fn get(&self, _id: &str) -> Result<Option<String>, String> {
        Err("此平台暂不支持凭据管理器（见 T-301/T-302）".into())
    }
    fn set(&self, _id: &str, _secret: &str) -> Result<(), String> {
        Err("此平台暂不支持凭据管理器（见 T-301/T-302）".into())
    }
    fn delete(&self, _id: &str) -> Result<(), String> {
        Err("此平台暂不支持凭据管理器（见 T-301/T-302）".into())
    }
}

/* --------------------------- Win32 advapi32 ---------------------------- */

#[cfg(windows)]
mod win {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;

    const CRED_TYPE_GENERIC: u32 = 1;
    const CRED_PERSIST_LOCAL_MACHINE: u32 = 2;

    #[repr(C)]
    struct CredentialW {
        flags: u32,
        cred_type: u32,
        target_name: *const u16,
        comment: *const u16,
        last_written: u64,
        credential_blob_size: u32,
        credential_blob: *const u8,
        persist: u32,
        attribute_count: u32,
        attributes: *mut core::ffi::c_void,
        target_alias: *const u16,
        user_name: *const u16,
    }

    extern "system" {
        fn CredWriteW(credential: *const CredentialW, flags: u32) -> i32;
        fn CredReadW(
            target_name: *const u16,
            cred_type: u32,
            flags: u32,
            credential: *mut *mut CredentialW,
        ) -> i32;
        fn CredDeleteW(target_name: *const u16, cred_type: u32, flags: u32) -> i32;
        fn CredFree(buffer: *mut core::ffi::c_void);
    }

    fn wide(value: &str) -> Vec<u16> {
        OsStr::new(value).encode_wide().chain([0]).collect()
    }

    fn last_error(context: &str) -> String {
        format!(
            "{context}: Win32 错误码 {}",
            std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
        )
    }

    pub fn read(target: &str) -> Result<Option<String>, String> {
        let target_w = wide(target);
        let mut credential: *mut CredentialW = std::ptr::null_mut();
        // A missing entry is `Ok(None)`, not an error: first-run providers
        // have no stored key yet.
        let ok = unsafe { CredReadW(target_w.as_ptr(), CRED_TYPE_GENERIC, 0, &mut credential) };
        if ok == 0 {
            const ERROR_NOT_FOUND: i32 = 1168;
            let code = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
            if code == ERROR_NOT_FOUND {
                return Ok(None);
            }
            return Err(last_error("读取凭据失败"));
        }
        let result = unsafe {
            let blob = (*credential).credential_blob;
            let size = (*credential).credential_blob_size as usize;
            let bytes = std::slice::from_raw_parts(blob, size);
            String::from_utf8(bytes.to_vec())
                .map(Some)
                .map_err(|_| "凭据内容不是有效的 UTF-8".to_string())
        };
        unsafe { CredFree(credential as *mut core::ffi::c_void) };
        result
    }

    pub fn write(target: &str, secret: &str) -> Result<(), String> {
        let target_w = wide(target);
        let blob = secret.as_bytes();
        let credential = CredentialW {
            flags: 0,
            cred_type: CRED_TYPE_GENERIC,
            target_name: target_w.as_ptr(),
            comment: std::ptr::null(),
            last_written: 0, // the OS stamps this
            credential_blob_size: blob.len() as u32,
            credential_blob: blob.as_ptr(),
            persist: CRED_PERSIST_LOCAL_MACHINE,
            attribute_count: 0,
            attributes: std::ptr::null_mut(),
            target_alias: std::ptr::null(),
            user_name: std::ptr::null(),
        };
        let ok = unsafe { CredWriteW(&credential, 0) };
        if ok == 0 {
            return Err(last_error("写入凭据失败"));
        }
        Ok(())
    }

    pub fn delete(target: &str) -> Result<(), String> {
        let target_w = wide(target);
        let ok = unsafe { CredDeleteW(target_w.as_ptr(), CRED_TYPE_GENERIC, 0) };
        if ok == 0 {
            const ERROR_NOT_FOUND: i32 = 1168;
            let code = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
            if code == ERROR_NOT_FOUND {
                return Ok(()); // deleting an absent key is already the goal
            }
            return Err(last_error("删除凭据失败"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Round-trips through the real OS store on Windows. The entry is
    /// namespaced and removed again — the test leaves nothing behind.
    #[test]
    #[cfg(windows)]
    fn credential_roundtrip_through_the_os_store() {
        let id = format!("test:{}", std::process::id());
        let store = Creds::platform_default();

        assert_eq!(store.get(&id).unwrap(), None, "clean slate");
        store.set(&id, "sk-test-abcdef").unwrap();
        assert_eq!(store.get(&id).unwrap().as_deref(), Some("sk-test-abcdef"));
        // Overwrite stores, not duplicates.
        store.set(&id, "sk-second").unwrap();
        assert_eq!(store.get(&id).unwrap().as_deref(), Some("sk-second"));
        store.delete(&id).unwrap();
        assert_eq!(store.get(&id).unwrap(), None);
        // Deleting an absent key is idempotent.
        store.delete(&id).unwrap();
    }

    #[test]
    fn provider_ids_are_namespaced() {
        assert_eq!(provider_credential_id("deepseek"), "PHL:provider:deepseek");
    }
}
