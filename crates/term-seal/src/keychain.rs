//! The key of every sealed file: a generic password in the login keychain, its service the bundle identifier (E19),
//! made on first use and readable only on this Mac.

use objc2_core_foundation::{CFBoolean, CFData, CFDictionary, CFRetained, CFString, CFType};
use objc2_security::{
    SecItemAdd, SecItemCopyMatching, kSecAttrAccessible, kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly,
    kSecAttrAccount, kSecAttrService, kSecClass, kSecClassGenericPassword, kSecReturnData, kSecValueData,
};

use crate::{KEY_LEN, SealError, new_key};

/// The keychain item's account, beside the service; named after its first user, the restored state.
const ACCOUNT: &str = "restore";

#[expect(non_upper_case_globals, reason = "the Security framework's names")]
const errSecSuccess: i32 = 0;
#[expect(non_upper_case_globals, reason = "the Security framework's names")]
const errSecItemNotFound: i32 = -25300;

/// The key from the keychain, made and stored when there is none yet.
pub fn key() -> Result<[u8; KEY_LEN], SealError> {
    if let Some(key) = stored()? {
        return Ok(key);
    }
    let key = new_key()?;
    add(&key)?;
    Ok(key)
}

fn service() -> CFRetained<CFString> {
    CFString::from_str(&term_base::identity().bundle_id)
}

fn stored() -> Result<Option<[u8; KEY_LEN]>, SealError> {
    let service = service();
    let account = CFString::from_str(ACCOUNT);
    let Some(yes) = true_value() else {
        return Err(SealError::System("kCFBooleanTrue", 0));
    };
    // SAFETY: The constants are the Security framework's, valid for the life of the process.
    let (keys, values): (Vec<&CFType>, Vec<&CFType>) = unsafe {
        (
            vec![kSecClass, kSecAttrService, kSecAttrAccount, kSecReturnData],
            vec![kSecClassGenericPassword, &service, &account, yes],
        )
    };
    let query = CFDictionary::<CFType, CFType>::from_slices(&keys, &values);
    let mut result: *const CFType = std::ptr::null();
    // SAFETY: The query holds CF keys and values as SecItem expects; `result` receives a retained CFData.
    let status = unsafe { SecItemCopyMatching(query.as_opaque(), &raw mut result) };
    if status == errSecItemNotFound {
        return Ok(None);
    }
    if status != errSecSuccess || result.is_null() {
        return Err(SealError::System("SecItemCopyMatching", status));
    }
    // SAFETY: A successful match with kSecReturnData hands over one retained CFData.
    let data: CFRetained<CFType> = unsafe { CFRetained::from_raw(std::ptr::NonNull::from(&*result)) };
    let bytes = data.downcast_ref::<CFData>().map(CFData::to_vec).unwrap_or_default();
    Ok(bytes.try_into().ok())
}

fn add(key: &[u8; KEY_LEN]) -> Result<(), SealError> {
    let service = service();
    let account = CFString::from_str(ACCOUNT);
    let data = CFData::from_bytes(key);
    // SAFETY: The constants are the Security framework's, valid for the life of the process.
    let (keys, values): (Vec<&CFType>, Vec<&CFType>) = unsafe {
        (
            vec![
                kSecClass,
                kSecAttrService,
                kSecAttrAccount,
                kSecValueData,
                kSecAttrAccessible,
            ],
            vec![
                kSecClassGenericPassword,
                &service,
                &account,
                &data,
                kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly,
            ],
        )
    };
    let attributes = CFDictionary::<CFType, CFType>::from_slices(&keys, &values);
    // SAFETY: The attributes hold CF keys and values as SecItem expects; no result is asked for.
    let status = unsafe { SecItemAdd(attributes.as_opaque(), std::ptr::null_mut()) };
    if status != errSecSuccess {
        return Err(SealError::System("SecItemAdd", status));
    }
    Ok(())
}

pub(crate) fn true_value() -> Option<&'static CFType> {
    // SAFETY: Core Foundation's constant, valid for the life of the process.
    unsafe { objc2_core_foundation::kCFBooleanTrue }.map(|yes: &CFBoolean| -> &CFType { yes })
}
