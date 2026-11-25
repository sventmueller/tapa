//! Copies the generic passwords kept under a former bundle identifier to the current one (E19), so a changed
//! identifier keeps the user's keys; the former items stay where they are.

use objc2_core_foundation::{CFArray, CFData, CFDictionary, CFRetained, CFString, CFType};
use objc2_security::{
    SecItemAdd, SecItemCopyMatching, kSecAttrAccessible, kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly,
    kSecAttrAccount, kSecAttrService, kSecClass, kSecClassGenericPassword, kSecMatchLimit, kSecMatchLimitAll,
    kSecReturnAttributes, kSecReturnData, kSecValueData,
};

use crate::secrets::SecretError;

#[expect(non_upper_case_globals, reason = "the Security framework's names")]
const errSecSuccess: i32 = 0;
#[expect(non_upper_case_globals, reason = "the Security framework's names")]
const errSecItemNotFound: i32 = -25300;

/// Copies every item of the service `former` whose account the current bundle identifier has none of yet, and
/// returns the accounts copied; reading each item may make the keychain ask the user once.
pub fn copy_from(former: &str) -> Result<Vec<String>, SecretError> {
    let current = term_base::identity().bundle_id.clone();
    if former == current {
        return Ok(Vec::new());
    }
    let wanted = missing(&accounts(former)?, &accounts(&current)?);
    let mut copied = Vec::new();
    for account in wanted {
        if let Some(data) = data(former, &account)? {
            add(&current, &account, &data)?;
            copied.push(account);
        }
    }
    Ok(copied)
}

/// The accounts of `former` that `current` lacks, in the order of `former`.
fn missing(former: &[String], current: &[String]) -> Vec<String> {
    former
        .iter()
        .filter(|account| !current.contains(account))
        .cloned()
        .collect()
}

fn query(service: &str, more: &[(&CFType, &CFType)]) -> CFRetained<CFDictionary<CFType, CFType>> {
    let service = CFString::from_str(service);
    // SAFETY: The constants are the Security framework's, valid for the life of the process.
    let (mut keys, mut values): (Vec<&CFType>, Vec<&CFType>) = unsafe {
        (
            vec![kSecClass, kSecAttrService],
            vec![kSecClassGenericPassword, &service],
        )
    };
    for (key, value) in more {
        keys.push(key);
        values.push(value);
    }
    CFDictionary::<CFType, CFType>::from_slices(&keys, &values)
}

/// The accounts of the items under `service`; listing them reads no secret, so the keychain does not ask.
fn accounts(service: &str) -> Result<Vec<String>, SecretError> {
    let Some(yes) = crate::keychain::true_value() else {
        return Err(SecretError("kCFBooleanTrue", 0));
    };
    // SAFETY: The constants are the Security framework's, valid for the life of the process.
    let query = unsafe {
        query(
            service,
            &[(kSecReturnAttributes, yes), (kSecMatchLimit, kSecMatchLimitAll)],
        )
    };
    let mut result: *const CFType = std::ptr::null();
    // SAFETY: The query holds CF keys and values as SecItem expects; `result` receives a retained CFArray.
    let status = unsafe { SecItemCopyMatching(query.as_opaque(), &raw mut result) };
    if status == errSecItemNotFound {
        return Ok(Vec::new());
    }
    if status != errSecSuccess || result.is_null() {
        return Err(SecretError("SecItemCopyMatching", status));
    }
    // SAFETY: A successful match with kSecMatchLimitAll hands over one retained CFArray.
    let found: CFRetained<CFType> = unsafe { CFRetained::from_raw(std::ptr::NonNull::from(&*result)) };
    let Some(items) = found.downcast_ref::<CFArray>() else {
        return Ok(Vec::new());
    };
    // SAFETY: The array SecItem returns holds CF objects, one attribute dictionary per item.
    let items: &CFArray<CFType> = unsafe { &*std::ptr::from_ref(items).cast() };
    // SAFETY: The Security framework's constant, valid for the life of the process.
    let account_key: &CFType = unsafe { kSecAttrAccount };
    Ok(items
        .iter()
        .filter_map(|item| {
            let item = item.downcast_ref::<CFDictionary>()?;
            // SAFETY: An attribute dictionary has CF keys and CF values.
            let item: &CFDictionary<CFType, CFType> = unsafe { &*std::ptr::from_ref(item).cast() };
            item.get(account_key)?
                .downcast_ref::<CFString>()
                .map(ToString::to_string)
        })
        .collect())
}

fn data(service: &str, account: &str) -> Result<Option<Vec<u8>>, SecretError> {
    let Some(yes) = crate::keychain::true_value() else {
        return Err(SecretError("kCFBooleanTrue", 0));
    };
    let account = CFString::from_str(account);
    // SAFETY: The constants are the Security framework's, valid for the life of the process.
    let query = unsafe { query(service, &[(kSecAttrAccount, &account), (kSecReturnData, yes)]) };
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
    Ok(data.downcast_ref::<CFData>().map(CFData::to_vec))
}

fn add(service: &str, account: &str, data: &[u8]) -> Result<(), SecretError> {
    let account = CFString::from_str(account);
    let data = CFData::from_bytes(data);
    // SAFETY: The constants are the Security framework's, valid for the life of the process.
    let attributes = unsafe {
        query(
            service,
            &[
                (kSecAttrAccount, &account),
                (kSecValueData, &data),
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

#[cfg(test)]
mod tests {
    use super::missing;

    #[test]
    fn only_accounts_the_current_service_lacks_are_copied() {
        let former = [
            "restore".to_string(),
            "dictation.openai".into(),
            "dictation.groq".into(),
        ];
        let current = ["dictation.openai".to_string()];
        assert_eq!(missing(&former, &current), ["restore", "dictation.groq"]);
        assert!(missing(&former, &former).is_empty());
        assert!(missing(&[], &current).is_empty());
    }
}
