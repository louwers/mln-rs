//! The default locale of the process that runs the tests. An expression that
//! names no locale takes the platform's default, which is the user's, and the
//! tests expect `en-US`.

#![expect(unsafe_code, reason = "Foundation through objc2, and the process environment")]

/// Makes `en-US` the default locale of the locale-sensitive expressions, for
/// the rest of the process. Call it first in `main` or in a test, before the
/// process evaluates an expression or starts a thread: the platforms read the
/// default once.
pub fn pin_default_locale() {
    #[cfg(target_vendor = "apple")]
    apple::pin();
    // SAFETY: the caller has yet to start a thread that reads the
    // environment.
    #[cfg(not(target_vendor = "apple"))]
    unsafe {
        std::env::set_var("LC_ALL", "en_US.UTF-8");
    }
}

#[cfg(target_vendor = "apple")]
mod apple {
    use objc2::rc::autoreleasepool;
    use objc2::runtime::AnyObject;
    use objc2_foundation::{NSArgumentDomain, NSDictionary, NSString, NSUserDefaults};

    /// Sets the `AppleLocale` user default, which names Foundation's current
    /// locale, in the argument domain. That domain is searched before the
    /// user's own, and stays in the process.
    pub(super) fn pin() {
        autoreleasepool(|_| {
            let key = NSString::from_str("AppleLocale");
            let value = NSString::from_str("en_US");
            let value: &AnyObject = &value;
            let domain = NSDictionary::from_slices(&[&*key], &[value]);
            let defaults = NSUserDefaults::standardUserDefaults();
            // SAFETY: the name is a volatile domain's, and the domain holds a
            // string, a property list value, under a string key.
            unsafe {
                defaults.removeVolatileDomainForName(NSArgumentDomain);
                defaults.setVolatileDomain_forName(&domain, NSArgumentDomain);
            }
        });
    }
}
