//! Secrets the user hands the app, such as the API keys of dictation: generic passwords in the login keychain under
//! the bundle identifier as their service (E19), one account per secret, or memory for tests and the self-test.
//!
//! ```
//! use term_seal::secrets::{Memory, SecretStore};
//!
//! let store = Memory::default();
//! store.set("dictation.openai", "sk-test").unwrap();
//! assert_eq!(store.get("dictation.openai").unwrap().as_deref(), Some("sk-test"));
//! store.delete("dictation.openai").unwrap();
//! assert_eq!(store.get("dictation.openai").unwrap(), None);
//! ```

use std::collections::HashMap;
use std::fmt;
use std::sync::Mutex;

use objc2_core_foundation::{CFData, CFDictionary, CFRetained, CFString, CFType};
use objc2_security::{
    SecItemAdd, SecItemCopyMatching, SecItemDelete, SecItemUpdate, kSecAttrAccessible,
    kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly, kSecAttrAccount, kSecAttrService, kSecClass,
    kSecClassGenericPassword, kSecReturnData, kSecValueData,
};
use term_base::LockIgnoringPoison;

#[expect(non_upper_case_globals, reason = "the Security framework's names")]
const errSecSuccess: i32 = 0;
#[expect(non_upper_case_globals, reason = "the Security framework's names")]
const errSecItemNotFound: i32 = -25300;

/// Why the keychain refused: the call and its status code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretError(pub &'static str, pub i32);

impl fmt::Display for SecretError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} failed with status {}", self.0, self.1)
    }
}

impl std::error::Error for SecretError {}

/// Where secrets are kept.
pub trait SecretStore: Send + Sync {
    /// The secret saved for `account`.
    fn get(&self, account: &str) -> Result<Option<String>, SecretError>;
    /// Saves `secret` for `account`, in place of what was there.
    fn set(&self, account: &str, secret: &str) -> Result<(), SecretError>;
    /// Forgets the secret of `account`; nothing to forget is no error.
    fn delete(&self, account: &str) -> Result<(), SecretError>;
}

/// The login keychain under the app's bundle identifier.
#[derive(Debug, Default)]
pub struct Keychain;

/// Secrets in memory, for tests and the self-test.
#[derive(Debug, Default)]
pub struct Memory(Mutex<HashMap<String, String>>);

impl SecretStore for Memory {
    fn get(&self, account: &str) -> Result<Option<String>, SecretError> {
        Ok(self.0.lock_ignoring_poison().get(account).cloned())
    }

    fn set(&self, account: &str, secret: &str) -> Result<(), SecretError> {
        self.0.lock_ignoring_poison().insert(account.into(), secret.into());
        Ok(())
    }

    fn delete(&self, account: &str) -> Result<(), SecretError> {
        self.0.lock_ignoring_poison().remove(account);
        Ok(())
    }
}

/// The query of `account`'s item, with `more` keys and values after the class, service and account.
fn query(account: &str, more: &[(&CFType, &CFType)]) -> CFRetained<CFDictionary<CFType, CFType>> {
    let service = CFString::from_str(&term_base::identity().bundle_id);
    let account = CFString::from_str(account);
    // SAFETY: The constants are the Security framework's, valid for the life of the process.
    let (mut keys, mut values): (Vec<&CFType>, Vec<&CFType>) = unsafe {
        (
            vec![kSecClass, kSecAttrService, kSecAttrAccount],
            vec![kSecClassGenericPassword, &service, &account],
        )
    };
    for (key, value) in more {
        keys.push(key);
        values.push(value);
    }
    CFDictionary::<CFType, CFType>::from_slices(&keys, &values)
}

impl SecretStore for Keychain {
    fn get(&self, account: &str) -> Result<Option<String>, SecretError> {
        let Some(yes) = crate::keychain::true_value() else {
            return Err(SecretError("kCFBooleanTrue", 0));
        };
        // SAFETY: The Security framework's constant, valid for the life of the process.
        let query = query(account, &[(unsafe { kSecReturnData }, yes)]);
        let mut result: *const CFType = std::ptr::null();
        // SAFETY: The query holds CF keys and values as SecItem expects; `result` receives a retained CFData.
        let status = unsafe { SecItemCopyMatching(query.as_opaque(), &raw mut result) };
        if status == errSecItemNotFound {
            return Ok(None);
        }
        if status != errSecSuccess || result.is_null() {
            return Err(SecretError("SecItemCopyMatching", status));
        }
        // SAFETY: A successful match with kSecReturnData hands over one retained CFData.
        let data: CFRetained<CFType> = unsafe { CFRetained::from_raw(std::ptr::NonNull::from(&*result)) };
        let bytes = data.downcast_ref::<CFData>().map(CFData::to_vec).unwrap_or_default();
        Ok(String::from_utf8(bytes).ok())
    }

    fn set(&self, account: &str, secret: &str) -> Result<(), SecretError> {
        let data = CFData::from_bytes(secret.as_bytes());
        // SAFETY: The Security framework's constant, valid for the life of the process.
        let value = unsafe { kSecValueData };
        let changes = CFDictionary::<CFType, CFType>::from_slices(&[value], &[&*data]);
        // SAFETY: Both dictionaries hold CF keys and values as SecItem expects.
        let status = unsafe { SecItemUpdate(query(account, &[]).as_opaque(), changes.as_opaque()) };
        if status == errSecSuccess {
            return Ok(());
        }
        if status != errSecItemNotFound {
            return Err(SecretError("SecItemUpdate", status));
        }
        // SAFETY: The constants are the Security framework's, valid for the life of the process.
        let attributes = unsafe {
            query(
                account,
                &[
                    (value, &*data),
                    (kSecAttrAccessible, kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly),
                ],
            )
        };
        // SAFETY: The attributes hold CF keys and values as SecItem expects; no result is asked for.
        let status = unsafe { SecItemAdd(attributes.as_opaque(), std::ptr::null_mut()) };
        if status != errSecSuccess {
            return Err(SecretError("SecItemAdd", status));
        }
        Ok(())
    }

    fn delete(&self, account: &str) -> Result<(), SecretError> {
        // SAFETY: The query holds CF keys and values as SecItem expects.
        let status = unsafe { SecItemDelete(query(account, &[]).as_opaque()) };
        if status == errSecSuccess || status == errSecItemNotFound {
            return Ok(());
        }
        Err(SecretError("SecItemDelete", status))
    }
}
