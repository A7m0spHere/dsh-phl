//! Credential storage (T-205/T-206): API keys live behind a `CredentialStore`
//! abstraction — on Windows, the OS Credential Manager (advapi32's
//! CredRead/CredWrite, *not* a home-grown crypto scheme); on macOS, the
//! system Keychain (generic passwords, T-301) — and the API
//! library keeps only references. Nothing here ever logs a secret.
//!
//! Resolution order everywhere a key is consumed (the launch-time injector
//! is the reference implementation): an environment variable the provider
//! names — inherited system env or an instance env — always wins; the
//! config's one-time key comes next; the credential store is the fallback
//! that makes "paste the key once in PHL" work for every instance.
//! Old `api.json` files that carry plaintext keys are migrated into
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
///
/// `allow(dead_code)`: Linux builds never call it (their store is the
/// unsupported stub); the Windows and macOS backends use this exact id as
/// their entry namespace, and the unit test below pins it on every platform.
#[allow(dead_code)]
pub fn provider_credential_id(provider_id: &str) -> String {
    // `PHL:` namespaces our entries inside the user's credential manager;
    // provider ids are already validated (`valid_provider_name`).
    format!("PHL:provider:{provider_id}")
}

/// The concrete store managed as Tauri state. Windows uses the OS Credential
/// Manager, macOS the system Keychain; Linux builds get a store that reports
/// unsupported (the platform gate T-302 carries that backend later).
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

#[cfg(target_os = "macos")]
impl CredentialStore for PlatformStore {
    fn get(&self, id: &str) -> Result<Option<String>, String> {
        macos::read(&provider_credential_id(id))
    }
    fn set(&self, id: &str, secret: &str) -> Result<(), String> {
        macos::write(&provider_credential_id(id), secret)
    }
    fn delete(&self, id: &str) -> Result<(), String> {
        macos::delete(&provider_credential_id(id))
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
impl CredentialStore for PlatformStore {
    fn get(&self, _id: &str) -> Result<Option<String>, String> {
        Err("此平台暂不支持凭据管理器（见 T-302）".into())
    }
    fn set(&self, _id: &str, _secret: &str) -> Result<(), String> {
        Err("此平台暂不支持凭据管理器（见 T-302）".into())
    }
    fn delete(&self, _id: &str) -> Result<(), String> {
        Err("此平台暂不支持凭据管理器（见 T-302）".into())
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

/* ---------------------------- macOS Keychain --------------------------- */

#[cfg(target_os = "macos")]
mod macos {
    use core_foundation::data::CFData;
    use security_framework::base::Error;
    use security_framework::item::{
        update_item, ItemAddOptions, ItemAddValue, ItemClass, ItemSearchOptions, ItemUpdateOptions,
        ItemUpdateValue, SearchResult,
    };

    /// Every PHL secret is a generic password under this one service name;
    /// the account is the full namespaced id (`PHL:provider:<id>`), so the
    /// Windows target name appears verbatim in Keychain Access and one
    /// namespace function stays correct on both platforms.
    const SERVICE: &str = "PHL";

    // SecItem status codes (errSecItemNotFound / errSecDuplicateItem).
    const ITEM_NOT_FOUND: i32 = -25300;
    const DUPLICATE_ITEM: i32 = -25299;

    fn err(context: &str, e: Error) -> String {
        let message = e
            .message()
            .unwrap_or_else(|| format!("状态码 {}", e.code()));
        format!("{context}: {message}")
    }

    /// The match dictionary for one entry. Deliberately **no** `limit` /
    /// `load_data` here: the same options are handed to `SecItemUpdate`, and
    /// Apple's docs are explicit that result-setting keys in the update's
    /// search dictionary fail with `ParamCore (-50)`.
    fn match_entry(target: &str) -> ItemSearchOptions {
        let mut s = ItemSearchOptions::new();
        s.class(ItemClass::generic_password())
            .service(SERVICE)
            .account(target);
        s
    }

    pub fn read(target: &str) -> Result<Option<String>, String> {
        let mut s = match_entry(target);
        s.load_data(true).limit(1);
        match s.search() {
            Ok(results) => match results.into_iter().next() {
                Some(SearchResult::Data(bytes)) => String::from_utf8(bytes)
                    .map(Some)
                    .map_err(|_| "凭据内容不是有效的 UTF-8".to_string()),
                // `load_data(true)` always yields `Data`; anything else is a
                // broken match, treated like a missing entry, not a crash.
                Some(_) => Ok(None),
                None => Ok(None),
            },
            // A missing entry is `Ok(None)`, not an error (same contract as
            // the Windows backend): first-run providers have no stored key.
            Err(e) if e.code() == ITEM_NOT_FOUND => Ok(None),
            Err(e) => Err(err("读取凭据失败", e)),
        }
    }

    pub fn write(target: &str, secret: &str) -> Result<(), String> {
        // Update first (rewrites the value in place); add on not-found.
        let mut update = ItemUpdateOptions::new();
        update.set_value(ItemUpdateValue::Data(CFData::from_buffer(
            secret.as_bytes(),
        )));
        match update_item(&match_entry(target), &update) {
            Ok(()) => Ok(()),
            Err(e) if e.code() == ITEM_NOT_FOUND => add(target, secret),
            Err(e) => Err(err("写入凭据失败", e)),
        }
    }

    fn add(target: &str, secret: &str) -> Result<(), String> {
        let mut add = ItemAddOptions::new(ItemAddValue::Data {
            class: ItemClass::generic_password(),
            data: CFData::from_buffer(secret.as_bytes()),
        });
        add.set_service(SERVICE)
            .set_account_name(target)
            .set_label("PHL 供应商 API 密钥");
        match add.add() {
            Ok(()) => Ok(()),
            // Lost an update/add race against ourselves; put the new value in.
            Err(e) if e.code() == DUPLICATE_ITEM => {
                let mut update = ItemUpdateOptions::new();
                update.set_value(ItemUpdateValue::Data(CFData::from_buffer(
                    secret.as_bytes(),
                )));
                update_item(&match_entry(target), &update).map_err(|e| err("写入凭据失败", e))
            }
            Err(e) => Err(err("写入凭据失败", e)),
        }
    }

    pub fn delete(target: &str) -> Result<(), String> {
        match match_entry(target).delete() {
            Ok(()) => Ok(()),
            // Deleting an absent key is already the goal (Windows parity).
            Err(e) if e.code() == ITEM_NOT_FOUND => Ok(()),
            Err(e) => Err(err("删除凭据失败", e)),
        }
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

    /// Round-trips through the real macOS Keychain, mirroring the Windows
    /// test. `#[ignore]`d on purpose: the first access from a freshly built
    /// binary pops the Keychain ACL prompt, which an unattended CI run cannot
    /// answer. Run explicitly during a Mac acceptance pass with
    /// `cargo test credential_roundtrip_on_the_keychain -- --ignored`.
    /// The entry is namespaced `test:` + pid and deleted again.
    #[test]
    #[cfg(target_os = "macos")]
    #[ignore = "touches the real Keychain; may raise a permission prompt"]
    fn credential_roundtrip_on_the_keychain() {
        let id = format!("test:{}", std::process::id());
        let store = Creds::platform_default();

        assert_eq!(store.get(&id).unwrap(), None, "clean slate");
        store.set(&id, "sk-test-abcdef").unwrap();
        assert_eq!(store.get(&id).unwrap().as_deref(), Some("sk-test-abcdef"));
        // Overwrite updates the entry in place, not a duplicate.
        store.set(&id, "sk-second").unwrap();
        assert_eq!(store.get(&id).unwrap().as_deref(), Some("sk-second"));
        store.delete(&id).unwrap();
        assert_eq!(store.get(&id).unwrap(), None);
        // Deleting an absent key is idempotent.
        store.delete(&id).unwrap();
    }
}
