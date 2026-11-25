//! Certificates the user accepts for a dev server whose certificate macOS does not trust, such as a self-signed one:
//! only on this Mac or a local test domain, only for the host and port that showed it, only that certificate, and
//! only until the app quits.

use std::cell::RefCell;
use std::collections::HashMap;

use objc2::rc::Retained;
use objc2::{ClassType, msg_send};
use objc2_core_foundation::{CFData, CFRetained};
use objc2_foundation::{NSURLAuthenticationChallenge, NSURLCredential, NSURLSessionAuthChallengeDisposition};
use objc2_security::SecTrust;

use super::address;

/// A server, as host and port.
type Server = (String, u16);

/// The certificates of development servers that macOS refused and those the user accepted, while the app runs.
#[derive(Default)]
pub(crate) struct Certificates {
    /// The certificate each development server showed last when macOS refused it, as Security's exceptions for it.
    refused: RefCell<HashMap<Server, CFRetained<CFData>>>,
    /// The certificates the user accepted.
    accepted: RefCell<HashMap<Server, CFRetained<CFData>>>,
}

fn with_refused<R>(f: impl FnOnce(&RefCell<HashMap<Server, CFRetained<CFData>>>) -> R) -> R {
    f(&super::host::state().certificates.refused)
}

fn with_accepted<R>(f: impl FnOnce(&RefCell<HashMap<Server, CFRetained<CFData>>>) -> R) -> R {
    f(&super::host::state().certificates.accepted)
}

/// Whether the user may accept an untrusted certificate of `host`: this Mac, `.localhost`, and the domains reserved
/// for local networks and tests (`.local`, `.test`), never a public name.
///
/// ```
/// use term_ui_browser::may_accept;
///
/// assert!(may_accept("shop.test"));
/// assert!(!may_accept("example.com"));
/// ```
pub fn may_accept(host: &str) -> bool {
    let host = normal_host(host);
    address::is_loopback(&host) || host == "::1" || [".local", ".test"].iter().any(|suffix| host.ends_with(suffix))
}

/// The server of `url`, with the default port of its scheme.
pub(crate) fn server_of(url: &str) -> Option<Server> {
    let origin = address::origin(url)?;
    let (scheme, authority) = origin.split_once("://")?;
    let (host, port) = address::split_port(authority)?;
    let port = match port {
        Some(port) => port.parse().ok()?,
        None if scheme == "https" => 443,
        None => 80,
    };
    Some((normal_host(host), port))
}

fn normal_host(host: &str) -> String {
    host.trim_start_matches('[').trim_end_matches(']').to_ascii_lowercase()
}

/// Whether the certificate `url`'s server showed last can be accepted.
pub(crate) fn can_accept(url: &str) -> bool {
    server_of(url).is_some_and(|server| with_refused(|refused| refused.borrow().contains_key(&server)))
}

/// Accepts the certificate `url`'s server showed last; whether there was one.
pub(crate) fn accept(url: &str) -> bool {
    let Some(server) = server_of(url) else { return false };
    let Some(exceptions) = with_refused(|refused| refused.borrow_mut().remove(&server)) else {
        return false;
    };
    with_accepted(|accepted| accepted.borrow_mut().insert(server, exceptions));
    true
}

/// The answer to a server trust challenge: a credential for an accepted certificate of a development server, else
/// macOS's own judgement, after noting a refused certificate so the user can accept it.
pub(crate) fn answer(
    challenge: &NSURLAuthenticationChallenge,
) -> (NSURLSessionAuthChallengeDisposition, Option<Retained<NSURLCredential>>) {
    let default = (NSURLSessionAuthChallengeDisposition::PerformDefaultHandling, None);
    let space = challenge.protectionSpace();
    let host = space.host().to_string();
    let Ok(port) = u16::try_from(space.port()) else {
        return default;
    };
    if !may_accept(&host) {
        return default;
    }
    // SAFETY: A server trust protection space has a trust object, valid while its challenge is.
    let trust: *mut SecTrust = unsafe { msg_send![&*space, serverTrust] };
    // SAFETY: Nil or a trust object owned by the protection space, which outlives this call.
    let Some(trust) = (unsafe { trust.as_ref() }) else {
        return default;
    };
    let server = (normal_host(&host), port);
    let accepted = with_accepted(|accepted| accepted.borrow().get(&server).cloned());
    if let Some(exceptions) = accepted {
        // SAFETY: The trust is valid during the challenge, the exceptions are Security's own for this server; a
        // changed certificate does not match them.
        let trusted =
            unsafe { trust.set_exceptions(Some(&exceptions)) && trust.evaluate_with_error(std::ptr::null_mut()) };
        if trusted {
            // SAFETY: A credential for the trust the challenge carries, which the class method retains.
            let credential: Retained<NSURLCredential> =
                unsafe { msg_send![NSURLCredential::class(), credentialForTrust: trust] };
            return (NSURLSessionAuthChallengeDisposition::UseCredential, Some(credential));
        }
    }
    // SAFETY: The trust is valid during the challenge; a null error pointer is allowed.
    if !unsafe { trust.evaluate_with_error(std::ptr::null_mut()) } {
        // SAFETY: Exceptions exist for a trust once evaluated.
        if let Some(exceptions) = unsafe { trust.exceptions() } {
            with_refused(|refused| refused.borrow_mut().insert(server, exceptions));
        }
    }
    default
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_this_mac_and_local_test_domains_may_be_accepted() {
        for host in [
            "localhost",
            "127.0.0.1",
            "[::1]",
            "::1",
            "app.localhost",
            "printer.local",
            "shop.test",
        ] {
            assert!(may_accept(host), "{host}");
        }
        for host in [
            "example.com",
            "192.168.1.10",
            "localhost.example.com",
            "test",
            "evil-local",
            "my.testing",
        ] {
            assert!(!may_accept(host), "{host}");
        }
    }

    #[test]
    fn a_server_is_its_host_and_port() {
        assert_eq!(server_of("https://LocalHost:5173/x"), Some(("localhost".into(), 5173)));
        assert_eq!(server_of("https://shop.test/"), Some(("shop.test".into(), 443)));
        assert_eq!(server_of("https://[::1]:8443/"), Some(("::1".into(), 8443)));
        assert_eq!(server_of("https://[::1]/"), Some(("::1".into(), 443)));
        assert_eq!(server_of("about:blank"), None);
    }
}
