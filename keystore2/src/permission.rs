// Copyright 2020, The Android Open Source Project
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! This crate provides access control primitives for Keystore 2.0.
//! It provides high level functions for checking permissions in the keystore2 and keystore2_key
//! SELinux classes based on the keystore2_selinux backend.
//! It also provides KeystorePerm and KeyPerm as convenience wrappers for the SELinux permission
//! defined by keystore2 and keystore2_key respectively.

use keystore_aidl_generated as aidl;

use std::cmp::PartialEq;
use std::convert::From;

use crate::error::Error as KsError;
use keystore2_selinux as selinux;

use anyhow::Context as AnyhowContext;

use selinux::Backend;

// Replace getcon with a mock in the test situation
#[cfg(not(test))]
use selinux::getcon;
#[cfg(test)]
use tests::test_getcon as getcon;

/// The below example wraps the enum MyPermission in the tuple struct `MyPerm` and implements
///  * `From<i32> for `MyPerm`, where each unknown numeric value is mapped to the given default,
///    here `None`
///  * `Into<MyPermission> for `MyPerm`
///  * `MyPerm::foo()` and `MyPerm::bar()` which construct MyPerm instances representing
///    `MyPermission::Foo` and `MyPermission::Bar` respectively.
///  * `MyPerm.to_selinux(&self)`, which returns the selinux string representation of the
///    represented permission.
///  * Tests in the given test namespace for each permision that check that the numeric
///    representations of MyPermission and MyPerm match. (TODO replace with static assert if
///    they become available.)
///
/// ## Special behavior
/// If the keyword `use` appears as an selinux name `use_` is used as identifier for the
/// constructor function (e.g. `MePerm::use_()`) but the string returned by `to_selinux` will
/// still be `"use"`.
///
/// ## Example
/// ```
/// #[i32]
/// enum MyPermission {
///     None = 0,
///     Foo = 1,
///     Bar = 2,
/// }
///
/// implement_permission!(
///     /// MyPerm documentation.
///     #[derive(Clone, Copy, Debug, PartialEq)]
///     MyPermission as MyPerm with default (None = 0, none)
///     and test namespace my_perm_tests {
///         Foo = 1,           selinux name: foo;
///         Bar = 2,           selinux name: bar;
///     }
/// );
/// ```
macro_rules! implement_permission {
    // This rule provides the public interface of the macro. And starts the preprocessing
    // recursion (see below).
    ($(#[$m:meta])* $t:ty as $name:ident with default ($($def:tt)*)
        and test namespace $tn:ident { $($element:tt)* })
    => {
        implement_permission!(@replace_use $($m)*, $t, $name, $tn, ($($def)*), [] , $($element)*);
    };


    // The following three rules recurse through the elements of the form
    // `<enum variant> = <integer_literal>, selinux name: <selinux_name>;`
    // preprocessing the input.

    // The first rule terminates the recursion and passes the processed arguments to the final
    // rule that spills out the implementation.
    (@replace_use $($m:meta)*, $t:ty, $name:ident, $tn:ident, ($($def:tt)*), [$($out:tt)*], ) => {
        implement_permission!(@end $($m)*, $t, $name, $tn, ($($def)*) { $($out)* } );
    };

    // The second rule is triggered if the selinux name of an element is literally `use`.
    // It produces the tuple `<enum variant> = <integer_literal>, use_, use;`
    // and appends it to the out list.
    (@replace_use $($m:meta)*, $t:ty, $name:ident, $tn:ident, ($($def:tt)*), [$($out:tt)*],
        $e_name:ident = $e_val:expr, selinux name: use; $($element:tt)*)
    => {
        implement_permission!(@replace_use $($m)*, $t, $name, $tn, ($($def)*),
                              [$($out)* $e_name = $e_val, use_, use;], $($element)*);
    };

    // The third rule is the default rule which replaces every input tuple with
    // `<enum variant> = <integer_literal>, <selinux_name>, <selinux_name>;`
    // and appends the result to the out list.
    (@replace_use $($m:meta)*, $t:ty, $name:ident, $tn:ident, ($($def:tt)*), [$($out:tt)*],
        $e_name:ident = $e_val:expr, selinux name: $e_str:ident; $($element:tt)*)
    => {
        implement_permission!(@replace_use $($m)*, $t, $name, $tn, ($($def)*),
                              [$($out)* $e_name = $e_val, $e_str, $e_str;], $($element)*);
    };

    (@end $($m:meta)*, $t:ty, $name:ident, $tn:ident,
        ($def_name:ident = $def:expr, $def_selinux_name:ident) {
            $($element_name:ident = $element_val:expr, $element_identifier:ident,
                $selinux_name:ident;)*
        })
    =>
    {
        $(#[$m])*
        pub struct $name($t);

        impl From<i32> for $name {
            fn from (p: i32) -> Self {
                match p {
                    $def => Self(<$t>::$def_name),
                    $($element_val => Self(<$t>::$element_name),)*
                    _ => Self(<$t>::$def_name),
                }
            }
        }

        impl Into<$t> for $name {
            fn into(self) -> $t {
                self.0
            }
        }

        impl $name {
            /// Returns a string representation of the permission as required by
            /// `selinux::check_access`.
            pub fn to_selinux(&self) -> &'static str {
                match self {
                    Self(<$t>::$def_name) => stringify!($def_selinux_name),
                    $(Self(<$t>::$element_name) => stringify!($selinux_name),)*
                }
            }

            /// Creates an instance representing a permission with the same name.
            pub const fn $def_selinux_name() -> Self { Self(<$t>::$def_name) }
            $(
                /// Creates an instance representing a permission with the same name.
                pub const fn $element_identifier() -> Self { Self(<$t>::$element_name) }
             )*
        }
        #[cfg(test)]
        mod $tn {
            use super::*;

            #[test]
            fn $def_selinux_name() {
                assert_eq!($name::$def_selinux_name(), (<$t>::$def_name as i32).into());
            }
            $(
                #[test]
                fn $element_identifier() {
                    assert_eq!($name::$element_identifier(), (<$t>::$element_name as i32).into());
                }
            )*
        }
    };


}

implement_permission!(
    /// KeyPerm provides a convenient abstraction from the SELinux class `keystore2_key`.
    /// At the same time it maps `KeyPermissions` from the Keystore 2.0 AIDL Grant interface to
    /// the SELinux permissions. With the implement_permission macro, we conveniently
    /// provide mappings between the wire type bit field values, the rust enum and the SELinux
    /// string representation.
    ///
    /// ## Example
    ///
    /// In this access check `KeyPerm::get_info().to_selinux()` would return the SELinux representation
    /// "info".
    /// ```
    /// selinux::check_access(source_context, target_context, "keystore2_key",
    ///                       KeyPerm::get_info().to_selinux());
    /// ```
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    aidl::KeyPermission as KeyPerm with default (None = 0, none)
    and test namespace key_perm_tests {
        Delete = 1,         selinux name: delete;
        GenUniqueId = 2,    selinux name: gen_unique_id;
        GetInfo = 4,        selinux name: get_info;
        Grant = 8,          selinux name: grant;
        List = 0x10,        selinux name: list;
        ManageBlob = 0x20,  selinux name: manage_blob;
        Rebind = 0x40,      selinux name: rebind;
        ReqForcedOp = 0x80, selinux name: req_forced_op;
        Update = 0x100,     selinux name: update;
        Use = 0x200,        selinux name: use;
        UseDevId = 0x400,   selinux name: use_dev_id;
    }
);

/// KeystorePermission defines values for the SELinux `keystore2` security class.
/// Countrary to `KeyPermission`, this enum is not generated by AIDL and need not be
/// wrapped by newtype pattern. But we conveniently use the implement_permission macro
/// to provide the same feature that we did for `KeyPermission` to this set of permissions.
#[repr(i32)]
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum KeystorePermission {
    /// `None` is not a permission that can ever be granted. It is not known to the SEPolicy.
    None = 0,
    /// Checked when a new auth token is installed.
    AddAuth = 1,
    /// Checked when an app is uninstalled or wiped.
    ClearNs = 2,
    /// Checked when the locked state of Keystore 2.0 is queried.
    GetState = 4,
    /// Checked when Keystore 2.0 gets locked.
    Lock = 8,
    /// Checked when Keystore 2.0 shall be reset.
    Reset = 0x10,
    /// Checked when Keystore 2.0 shall be unlocked.
    Unlock = 0x20,
}

implement_permission!(
    /// KeystorePerm provides a convenient abstraction from the SELinux class `keystore2`.
    /// Using the implement_permission macro we get the same features as `KeyPerm`.
    #[derive(Clone, Copy, Debug, PartialEq)]
    KeystorePermission as KeystorePerm with default (None = 0, none)
    and test namespace keystore_perm_tests {
        AddAuth = 1,    selinux name: add_auth;
        ClearNs = 2,    selinux name: clear_ns;
        GetState = 4,   selinux name: get_state;
        Lock = 8,       selinux name: lock;
        Reset = 0x10,   selinux name: reset;
        Unlock = 0x20,  selinux name: unlock;
    }
);

/// Represents a set of `KeyPerm` permissions.
/// `IntoIterator` is implemented for this struct allowing the iteration through all the
/// permissions in the set.
/// It also implements a function `includes(self, other)` that checks if the permissions
/// in `other` are included in `self`.
///
/// KeyPermSet can be created with the macro `key_perm_set![]`.
///
/// ## Example
/// ```
/// let perms1 = key_perm_set![KeyPerm::use_(), KeyPerm::manage_blob(), KeyPerm::grant()];
/// let perms2 = key_perm_set![KeyPerm::use_(), KeyPerm::manage_blob()];
///
/// assert!(perms1.includes(perms2))
/// assert!(!perms2.includes(perms1))
///
/// let i = perms1.into_iter();
/// // iteration in ascending order of the permission's numeric representation.
/// assert_eq(Some(KeyPerm::manage_blob()), i.next());
/// assert_eq(Some(KeyPerm::grant()), i.next());
/// assert_eq(Some(KeyPerm::use_()), i.next());
/// assert_eq(None, i.next());
/// ```
#[derive(Copy, Clone)]
pub struct KeyPermSet(i32);

mod perm {
    use super::*;

    pub struct IntoIter {
        vec: KeyPermSet,
        pos: u8,
    }

    impl IntoIter {
        pub fn new(v: KeyPermSet) -> Self {
            Self { vec: v, pos: 0 }
        }
    }

    impl std::iter::Iterator for IntoIter {
        type Item = KeyPerm;

        fn next(&mut self) -> Option<Self::Item> {
            loop {
                if self.pos == 32 {
                    return None;
                }
                let p = self.vec.0 & (1 << self.pos);
                self.pos += 1;
                if p != 0 {
                    return Some(KeyPerm::from(p));
                }
            }
        }
    }
}

impl From<KeyPerm> for KeyPermSet {
    fn from(p: KeyPerm) -> Self {
        Self(p.0 as i32)
    }
}

impl KeyPermSet {
    /// Returns true iff this permission set has all of the permissions that are in `other`.
    fn includes<T: Into<KeyPermSet>>(&self, other: T) -> bool {
        let o: KeyPermSet = other.into();
        (self.0 & o.0) == o.0
    }
}

/// This macro can be used to create a `KeyPermSet` from a list of `KeyPerm` values.
///
/// ## Example
/// ```
/// let v = key_perm_set![Perm::delete(), Perm::manage_blob()];
/// ```
#[macro_export]
macro_rules! key_perm_set {
    () => { KeyPermSet(0) };
    ($head:expr $(, $tail:expr)* $(,)?) => {
        KeyPermSet($head.0 as i32 $(| $tail.0 as i32)*)
    };
}

impl IntoIterator for KeyPermSet {
    type Item = KeyPerm;
    type IntoIter = perm::IntoIter;

    fn into_iter(self) -> Self::IntoIter {
        Self::IntoIter::new(self)
    }
}

/// Uses `selinux::check_access` to check if the given caller context `caller_cxt` may access
/// the given permision `perm` of the `keystore2` security class.
pub fn check_keystore_permission(
    caller_ctx: &selinux::Context,
    perm: KeystorePerm,
) -> anyhow::Result<()> {
    let target_context = getcon().context("check_keystore_permission: getcon failed.")?;
    selinux::check_access(caller_ctx, &target_context, "keystore2", perm.to_selinux())
}

/// Uses `selinux::check_access` to check if the given caller context `caller_cxt` has
/// all the permissions indicated in `access_vec` for the target domain indicated by the key
/// descriptor `key` in the security class `keystore2_key`.
///
/// Also checks if the caller has the grant permission for the given target domain.
///
/// Attempts to grant the grant permission are always denied.
///
/// The only viable target domains are
///  * `Domain::App` in which case u:r:keystore:s0 is used as target context and
///  * `Domain::SELinux` in which case the `key.namespace_` parameter is looked up in
///                      SELinux keystore key backend, and the result is used
///                      as target context.
pub fn check_grant_permission(
    caller_ctx: &selinux::Context,
    access_vec: KeyPermSet,
    key: &aidl::KeyDescriptor,
) -> anyhow::Result<()> {
    use aidl::Domain;
    use selinux::KeystoreKeyBackend;

    let target_context = match key.domain {
        Domain::App => getcon().context("check_grant_permission: getcon failed.")?,
        Domain::SELinux => {
            // TODO cache an open backend, possible use a lazy static.
            let backend = KeystoreKeyBackend::new().context(concat!(
                "check_grant_permission: Domain::SELinux: ",
                "Failed to create selinux keystore backend."
            ))?;
            backend
                .lookup(format!("{}", key.namespace_).as_str())
                .context("check_grant_permission: Domain::SELinux: Failed to lookup namespace")?
        }
        _ => return Err(KsError::sys()).context(format!("Cannot grant {:?}.", key.domain)),
    };

    selinux::check_access(caller_ctx, &target_context, "keystore2_key", "grant")
        .context("Grant permission is required when granting.")?;

    if access_vec.includes(KeyPerm::grant()) {
        return Err(selinux::Error::perm()).context("Grant permission cannot be granted.");
    }

    for p in access_vec.into_iter() {
        selinux::check_access(caller_ctx, &target_context, "keystore2_key", p.to_selinux())
            .context(concat!(
                "check_grant_permission: check_access failed. ",
                "The caller may have tried to grant a permission that they don't possess."
            ))?
    }
    Ok(())
}

/// Uses `selinux::check_access` to check if the given caller context `caller_cxt`
/// has the permissions indicated by `perm` for the target domain indicated by the key
/// descriptor `key` in the security class `keystore2_key`.
///
/// The behavior differs slightly depending on the selected target domain:
///  * `Domain::App` u:r:keystore:s0 is used as target context.
///  * `Domain::SELinux` `key.namespace_` parameter is looked up in the SELinux keystore key
///                      backend, and the result is used as target context.
///  * `Domain::Blob` Same as SELinux but the "manage_blob" permission is always checked additionally
///                   to the one supplied in `perm`.
///  * `Domain::Grant` Does not use selinux::check_access. Instead the `access_vector`
///                    parameter is queried for permission, which must be supplied in this case.
///
/// ## Return values.
///  * Ok(()) If the requested permissions were granted.
///  * Err(selinux::Error::perm()) If the requested permissions were denied.
///  * Err(KsError::sys()) This error is produced if `Domain::Grant` is selected but no `access_vec`
///                      was supplied. It is also produced if `Domain::KeyId` was selected, and
///                      on various unexpected backend failures.
pub fn check_key_permission(
    caller_ctx: &selinux::Context,
    perm: KeyPerm,
    key: &aidl::KeyDescriptor,
    access_vector: &Option<KeyPermSet>,
) -> anyhow::Result<()> {
    use aidl::Domain;
    use selinux::KeystoreKeyBackend;

    let target_context = match key.domain {
        // apps get the default keystore context
        Domain::App => getcon().context("check_key_permission: getcon failed.")?,
        Domain::SELinux => {
            // TODO cache an open backend, possible use a lasy static.
            let backend = KeystoreKeyBackend::new().context(
                "check_key_permission: Domain::SELinux: Failed to create selinux keystore backend.",
            )?;
            backend
                .lookup(format!("{}", key.namespace_).as_str())
                .context("check_key_permission: Domain::SELinux: Failed to lookup namespace")?
        }
        Domain::Grant => {
            match access_vector {
                Some(pv) => {
                    if pv.includes(perm) {
                        return Ok(());
                    } else {
                        return Err(selinux::Error::perm())
                            .context(format!("\"{}\" not granted", perm.to_selinux()));
                    }
                }
                None => {
                    // If DOMAIN_GRANT was selected an access vector must be supplied.
                    return Err(KsError::sys()).context(
                        "Cannot check permission for Domain::Grant without access vector.",
                    );
                }
            }
        }
        Domain::KeyId => {
            // We should never be called with `Domain::KeyId. The database
            // lookup should have converted this into one of `Domain::App`
            // or `Domain::SELinux`.
            return Err(KsError::sys()).context("Cannot check permission for Domain::KeyId.");
        }
        Domain::Blob => {
            let backend = KeystoreKeyBackend::new()
                .context("Domain::Blob: Failed to create selinux keystore backend.")?;
            let tctx = backend
                .lookup(format!("{}", key.namespace_).as_str())
                .context("Domain::Blob: Failed to lookup namespace.")?;
            // If DOMAIN_KEY_BLOB was specified, we check for the "manage_blob"
            // permission in addition to the requested permission.
            selinux::check_access(
                caller_ctx,
                &tctx,
                "keystore2_key",
                KeyPerm::manage_blob().to_selinux(),
            )?;

            tctx
        }
    };

    selinux::check_access(caller_ctx, &target_context, "keystore2_key", perm.to_selinux())
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::anyhow;
    use anyhow::Result;
    use keystore2_selinux::*;
    use keystore_aidl_generated as aidl;

    const ALL_PERMS: KeyPermSet = key_perm_set![
        KeyPerm::manage_blob(),
        KeyPerm::delete(),
        KeyPerm::use_dev_id(),
        KeyPerm::req_forced_op(),
        KeyPerm::gen_unique_id(),
        KeyPerm::grant(),
        KeyPerm::get_info(),
        KeyPerm::list(),
        KeyPerm::rebind(),
        KeyPerm::update(),
        KeyPerm::use_(),
    ];

    const NOT_GRANT_PERMS: KeyPermSet = key_perm_set![
        KeyPerm::manage_blob(),
        KeyPerm::delete(),
        KeyPerm::use_dev_id(),
        KeyPerm::req_forced_op(),
        KeyPerm::gen_unique_id(),
        // No KeyPerm::grant()
        KeyPerm::get_info(),
        KeyPerm::list(),
        KeyPerm::rebind(),
        KeyPerm::update(),
        KeyPerm::use_(),
    ];

    const UNPRIV_PERMS: KeyPermSet = key_perm_set![
        KeyPerm::delete(),
        KeyPerm::get_info(),
        KeyPerm::list(),
        KeyPerm::rebind(),
        KeyPerm::update(),
        KeyPerm::use_(),
    ];

    /// The su_key namespace as defined in su.te and keystore_key_contexts of the
    /// SePolicy (system/sepolicy).
    const SU_KEY_NAMESPACE: i32 = 0;
    /// The shell_key namespace as defined in shell.te and keystore_key_contexts of the
    /// SePolicy (system/sepolicy).
    const SHELL_KEY_NAMESPACE: i32 = 1;

    pub fn test_getcon() -> Result<Context> {
        Context::new("u:object_r:keystore:s0")
    }

    // This macro evaluates the given expression and checks that
    // a) evaluated to Result::Err() and that
    // b) the wrapped error is selinux::Error::perm() (permission denied).
    // We use a macro here because a function would mask which invocation caused the failure.
    //
    // TODO b/164121720 Replace this macro with a function when `track_caller` is available.
    macro_rules! assert_perm_failed {
        ($test_function:expr) => {
            let result = $test_function;
            assert!(result.is_err(), "Permission check should have failed.");
            assert_eq!(
                Some(&selinux::Error::perm()),
                result.err().unwrap().root_cause().downcast_ref::<selinux::Error>()
            );
        };
    }

    fn check_context() -> Result<(selinux::Context, i32, bool)> {
        // Calling the non mocked selinux::getcon here intended.
        let context = selinux::getcon()?;
        match context.to_str().unwrap() {
            "u:r:su:s0" => Ok((context, SU_KEY_NAMESPACE, true)),
            "u:r:shell:s0" => Ok((context, SHELL_KEY_NAMESPACE, false)),
            c => Err(anyhow!(format!(
                "This test must be run as \"su\" or \"shell\". Current context: \"{}\"",
                c
            ))),
        }
    }

    #[test]
    fn check_keystore_permission_test() -> Result<()> {
        let system_server_ctx = Context::new("u:r:system_server:s0")?;
        assert!(check_keystore_permission(&system_server_ctx, KeystorePerm::add_auth()).is_ok());
        assert!(check_keystore_permission(&system_server_ctx, KeystorePerm::clear_ns()).is_ok());
        assert!(check_keystore_permission(&system_server_ctx, KeystorePerm::get_state()).is_ok());
        assert!(check_keystore_permission(&system_server_ctx, KeystorePerm::lock()).is_ok());
        assert!(check_keystore_permission(&system_server_ctx, KeystorePerm::reset()).is_ok());
        assert!(check_keystore_permission(&system_server_ctx, KeystorePerm::unlock()).is_ok());
        let shell_ctx = Context::new("u:r:shell:s0")?;
        assert_perm_failed!(check_keystore_permission(&shell_ctx, KeystorePerm::add_auth()));
        assert_perm_failed!(check_keystore_permission(&shell_ctx, KeystorePerm::clear_ns()));
        assert_perm_failed!(check_keystore_permission(&shell_ctx, KeystorePerm::get_state()));
        assert_perm_failed!(check_keystore_permission(&shell_ctx, KeystorePerm::lock()));
        assert_perm_failed!(check_keystore_permission(&shell_ctx, KeystorePerm::reset()));
        assert_perm_failed!(check_keystore_permission(&shell_ctx, KeystorePerm::unlock()));
        Ok(())
    }

    #[test]
    fn check_grant_permission_app() -> Result<()> {
        let system_server_ctx = Context::new("u:r:system_server:s0")?;
        let shell_ctx = Context::new("u:r:shell:s0")?;
        use aidl::Domain;
        let key =
            aidl::KeyDescriptor { domain: Domain::App, namespace_: 0, alias: None, blob: None };
        assert!(check_grant_permission(&system_server_ctx, NOT_GRANT_PERMS, &key).is_ok());
        // attempts to grant the grant permission must always fail even when privileged.

        assert_perm_failed!(check_grant_permission(
            &system_server_ctx,
            KeyPerm::grant().into(),
            &key
        ));
        // unprivileged grant attempts always fail. shell does not have the grant permission.
        assert_perm_failed!(check_grant_permission(&shell_ctx, UNPRIV_PERMS, &key));
        Ok(())
    }

    #[test]
    fn check_grant_permission_selinux() -> Result<()> {
        use aidl::Domain;
        let (sctx, namespace, is_su) = check_context()?;
        let key = aidl::KeyDescriptor {
            domain: Domain::SELinux,
            namespace_: namespace as i64,
            alias: None,
            blob: None,
        };
        if is_su {
            assert!(check_grant_permission(&sctx, NOT_GRANT_PERMS, &key).is_ok());
            // attempts to grant the grant permission must always fail even when privileged.
            assert_perm_failed!(check_grant_permission(&sctx, KeyPerm::grant().into(), &key));
        } else {
            // unprivileged grant attempts always fail. shell does not have the grant permission.
            assert_perm_failed!(check_grant_permission(&sctx, UNPRIV_PERMS, &key));
        }
        Ok(())
    }

    #[test]
    fn check_key_permission_domain_grant() -> Result<()> {
        use aidl::Domain;
        let key =
            aidl::KeyDescriptor { domain: Domain::Grant, namespace_: 0, alias: None, blob: None };

        assert_perm_failed!(check_key_permission(
            &selinux::Context::new("ignored").unwrap(),
            KeyPerm::grant(),
            &key,
            &Some(UNPRIV_PERMS)
        ));

        check_key_permission(
            &selinux::Context::new("ignored").unwrap(),
            KeyPerm::use_(),
            &key,
            &Some(ALL_PERMS),
        )
    }

    #[test]
    fn check_key_permission_domain_app() -> Result<()> {
        let system_server_ctx = Context::new("u:r:system_server:s0")?;
        let shell_ctx = Context::new("u:r:shell:s0")?;
        let gmscore_app = Context::new("u:r:gmscore_app:s0")?;
        use aidl::Domain;

        let key =
            aidl::KeyDescriptor { domain: Domain::App, namespace_: 0, alias: None, blob: None };

        assert!(check_key_permission(&system_server_ctx, KeyPerm::use_(), &key, &None).is_ok());
        assert!(check_key_permission(&system_server_ctx, KeyPerm::delete(), &key, &None).is_ok());
        assert!(check_key_permission(&system_server_ctx, KeyPerm::get_info(), &key, &None).is_ok());
        assert!(check_key_permission(&system_server_ctx, KeyPerm::rebind(), &key, &None).is_ok());
        assert!(check_key_permission(&system_server_ctx, KeyPerm::list(), &key, &None).is_ok());
        assert!(check_key_permission(&system_server_ctx, KeyPerm::update(), &key, &None).is_ok());
        assert!(check_key_permission(&system_server_ctx, KeyPerm::grant(), &key, &None).is_ok());
        assert!(
            check_key_permission(&system_server_ctx, KeyPerm::use_dev_id(), &key, &None).is_ok()
        );
        assert!(check_key_permission(&gmscore_app, KeyPerm::gen_unique_id(), &key, &None).is_ok());

        assert!(check_key_permission(&shell_ctx, KeyPerm::use_(), &key, &None).is_ok());
        assert!(check_key_permission(&shell_ctx, KeyPerm::delete(), &key, &None).is_ok());
        assert!(check_key_permission(&shell_ctx, KeyPerm::get_info(), &key, &None).is_ok());
        assert!(check_key_permission(&shell_ctx, KeyPerm::rebind(), &key, &None).is_ok());
        assert!(check_key_permission(&shell_ctx, KeyPerm::list(), &key, &None).is_ok());
        assert!(check_key_permission(&shell_ctx, KeyPerm::update(), &key, &None).is_ok());
        assert_perm_failed!(check_key_permission(&shell_ctx, KeyPerm::grant(), &key, &None));
        assert_perm_failed!(check_key_permission(
            &shell_ctx,
            KeyPerm::req_forced_op(),
            &key,
            &None
        ));
        assert_perm_failed!(check_key_permission(&shell_ctx, KeyPerm::manage_blob(), &key, &None));
        assert_perm_failed!(check_key_permission(&shell_ctx, KeyPerm::use_dev_id(), &key, &None));
        assert_perm_failed!(check_key_permission(
            &shell_ctx,
            KeyPerm::gen_unique_id(),
            &key,
            &None
        ));

        Ok(())
    }

    #[test]
    fn check_key_permission_domain_selinux() -> Result<()> {
        use aidl::Domain;
        let (sctx, namespace, is_su) = check_context()?;
        let key = aidl::KeyDescriptor {
            domain: Domain::SELinux,
            namespace_: namespace as i64,
            alias: None,
            blob: None,
        };

        if is_su {
            assert!(check_key_permission(&sctx, KeyPerm::use_(), &key, &None).is_ok());
            assert!(check_key_permission(&sctx, KeyPerm::delete(), &key, &None).is_ok());
            assert!(check_key_permission(&sctx, KeyPerm::get_info(), &key, &None).is_ok());
            assert!(check_key_permission(&sctx, KeyPerm::rebind(), &key, &None).is_ok());
            assert!(check_key_permission(&sctx, KeyPerm::list(), &key, &None).is_ok());
            assert!(check_key_permission(&sctx, KeyPerm::update(), &key, &None).is_ok());
            assert!(check_key_permission(&sctx, KeyPerm::grant(), &key, &None).is_ok());
            assert!(check_key_permission(&sctx, KeyPerm::manage_blob(), &key, &None).is_ok());
            assert!(check_key_permission(&sctx, KeyPerm::use_dev_id(), &key, &None).is_ok());
            assert!(check_key_permission(&sctx, KeyPerm::gen_unique_id(), &key, &None).is_ok());
            assert!(check_key_permission(&sctx, KeyPerm::req_forced_op(), &key, &None).is_ok());
        } else {
            assert!(check_key_permission(&sctx, KeyPerm::use_(), &key, &None).is_ok());
            assert!(check_key_permission(&sctx, KeyPerm::delete(), &key, &None).is_ok());
            assert!(check_key_permission(&sctx, KeyPerm::get_info(), &key, &None).is_ok());
            assert!(check_key_permission(&sctx, KeyPerm::rebind(), &key, &None).is_ok());
            assert!(check_key_permission(&sctx, KeyPerm::list(), &key, &None).is_ok());
            assert!(check_key_permission(&sctx, KeyPerm::update(), &key, &None).is_ok());
            assert_perm_failed!(check_key_permission(&sctx, KeyPerm::grant(), &key, &None));
            assert_perm_failed!(check_key_permission(&sctx, KeyPerm::req_forced_op(), &key, &None));
            assert_perm_failed!(check_key_permission(&sctx, KeyPerm::manage_blob(), &key, &None));
            assert_perm_failed!(check_key_permission(&sctx, KeyPerm::use_dev_id(), &key, &None));
            assert_perm_failed!(check_key_permission(&sctx, KeyPerm::gen_unique_id(), &key, &None));
        }
        Ok(())
    }

    #[test]
    fn check_key_permission_domain_blob() -> Result<()> {
        use aidl::Domain;
        let (sctx, namespace, is_su) = check_context()?;
        let key = aidl::KeyDescriptor {
            domain: Domain::Blob,
            namespace_: namespace as i64,
            alias: None,
            blob: None,
        };

        if is_su {
            check_key_permission(&sctx, KeyPerm::use_(), &key, &None)
        } else {
            assert_perm_failed!(check_key_permission(&sctx, KeyPerm::use_(), &key, &None));
            Ok(())
        }
    }

    #[test]
    fn check_key_permission_domain_key_id() -> Result<()> {
        use aidl::Domain;
        let key =
            aidl::KeyDescriptor { domain: Domain::KeyId, namespace_: 0, alias: None, blob: None };

        assert_eq!(
            Some(&KsError::sys()),
            check_key_permission(
                &selinux::Context::new("ignored").unwrap(),
                KeyPerm::use_(),
                &key,
                &None
            )
            .err()
            .unwrap()
            .root_cause()
            .downcast_ref::<KsError>()
        );
        Ok(())
    }

    #[test]
    fn key_perm_set_all_test() {
        let v = key_perm_set![
            KeyPerm::manage_blob(),
            KeyPerm::delete(),
            KeyPerm::use_dev_id(),
            KeyPerm::req_forced_op(),
            KeyPerm::gen_unique_id(),
            KeyPerm::grant(),
            KeyPerm::get_info(),
            KeyPerm::list(),
            KeyPerm::rebind(),
            KeyPerm::update(),
            KeyPerm::use_() // Test if the macro accepts missing comma at the end of the list.
        ];
        let mut i = v.into_iter();
        assert_eq!(i.next().unwrap().to_selinux(), "delete");
        assert_eq!(i.next().unwrap().to_selinux(), "gen_unique_id");
        assert_eq!(i.next().unwrap().to_selinux(), "get_info");
        assert_eq!(i.next().unwrap().to_selinux(), "grant");
        assert_eq!(i.next().unwrap().to_selinux(), "list");
        assert_eq!(i.next().unwrap().to_selinux(), "manage_blob");
        assert_eq!(i.next().unwrap().to_selinux(), "rebind");
        assert_eq!(i.next().unwrap().to_selinux(), "req_forced_op");
        assert_eq!(i.next().unwrap().to_selinux(), "update");
        assert_eq!(i.next().unwrap().to_selinux(), "use");
        assert_eq!(i.next().unwrap().to_selinux(), "use_dev_id");
        assert_eq!(None, i.next());
    }
    #[test]
    fn key_perm_set_sparse_test() {
        let v = key_perm_set![
            KeyPerm::manage_blob(),
            KeyPerm::req_forced_op(),
            KeyPerm::gen_unique_id(),
            KeyPerm::list(),
            KeyPerm::update(),
            KeyPerm::use_(), // Test if macro accepts the comma at the end of the list.
        ];
        let mut i = v.into_iter();
        assert_eq!(i.next().unwrap().to_selinux(), "gen_unique_id");
        assert_eq!(i.next().unwrap().to_selinux(), "list");
        assert_eq!(i.next().unwrap().to_selinux(), "manage_blob");
        assert_eq!(i.next().unwrap().to_selinux(), "req_forced_op");
        assert_eq!(i.next().unwrap().to_selinux(), "update");
        assert_eq!(i.next().unwrap().to_selinux(), "use");
        assert_eq!(None, i.next());
    }
    #[test]
    fn key_perm_set_empty_test() {
        let v = key_perm_set![];
        let mut i = v.into_iter();
        assert_eq!(None, i.next());
    }
    #[test]
    fn key_perm_set_include_subset_test() {
        let v1 = key_perm_set![
            KeyPerm::manage_blob(),
            KeyPerm::delete(),
            KeyPerm::use_dev_id(),
            KeyPerm::req_forced_op(),
            KeyPerm::gen_unique_id(),
            KeyPerm::grant(),
            KeyPerm::get_info(),
            KeyPerm::list(),
            KeyPerm::rebind(),
            KeyPerm::update(),
            KeyPerm::use_(),
        ];
        let v2 = key_perm_set![
            KeyPerm::manage_blob(),
            KeyPerm::delete(),
            KeyPerm::list(),
            KeyPerm::rebind(),
            KeyPerm::update(),
            KeyPerm::use_(),
        ];
        assert!(v1.includes(v2));
        assert!(!v2.includes(v1));
    }
    #[test]
    fn key_perm_set_include_equal_test() {
        let v1 = key_perm_set![
            KeyPerm::manage_blob(),
            KeyPerm::delete(),
            KeyPerm::list(),
            KeyPerm::rebind(),
            KeyPerm::update(),
            KeyPerm::use_(),
        ];
        let v2 = key_perm_set![
            KeyPerm::manage_blob(),
            KeyPerm::delete(),
            KeyPerm::list(),
            KeyPerm::rebind(),
            KeyPerm::update(),
            KeyPerm::use_(),
        ];
        assert!(v1.includes(v2));
        assert!(v2.includes(v1));
    }
    #[test]
    fn key_perm_set_include_overlap_test() {
        let v1 = key_perm_set![
            KeyPerm::manage_blob(),
            KeyPerm::delete(),
            KeyPerm::grant(), // only in v1
            KeyPerm::list(),
            KeyPerm::rebind(),
            KeyPerm::update(),
            KeyPerm::use_(),
        ];
        let v2 = key_perm_set![
            KeyPerm::manage_blob(),
            KeyPerm::delete(),
            KeyPerm::req_forced_op(), // only in v2
            KeyPerm::list(),
            KeyPerm::rebind(),
            KeyPerm::update(),
            KeyPerm::use_(),
        ];
        assert!(!v1.includes(v2));
        assert!(!v2.includes(v1));
    }
    #[test]
    fn key_perm_set_include_no_overlap_test() {
        let v1 = key_perm_set![KeyPerm::manage_blob(), KeyPerm::delete(), KeyPerm::grant(),];
        let v2 = key_perm_set![
            KeyPerm::req_forced_op(),
            KeyPerm::list(),
            KeyPerm::rebind(),
            KeyPerm::update(),
            KeyPerm::use_(),
        ];
        assert!(!v1.includes(v2));
        assert!(!v2.includes(v1));
    }
}
