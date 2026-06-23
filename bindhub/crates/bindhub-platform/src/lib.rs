//! Bindhub hosted platform boundary.
//!
//! Bindhub owns accounts, machines, membership, hosted discovery, and product
//! configuration. Loom owns folder state and sync semantics.

use loom_core::SharedFolderId;
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlatformError {
    EmptyId { kind: &'static str },
    EmptyName { kind: &'static str },
}

impl fmt::Display for PlatformError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyId { kind } => write!(f, "{kind} cannot be empty"),
            Self::EmptyName { kind } => write!(f, "{kind} cannot be empty"),
        }
    }
}

impl std::error::Error for PlatformError {}

macro_rules! platform_id {
    ($name:ident, $kind:literal) => {
        #[derive(Debug, Clone, PartialEq, Eq, Hash)]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Result<Self, PlatformError> {
                let value = value.into();
                if value.trim().is_empty() {
                    return Err(PlatformError::EmptyId { kind: $kind });
                }
                Ok(Self(value))
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }
    };
}

platform_id!(AccountId, "account id");
platform_id!(DeviceId, "device id");

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Machine {
    pub id: DeviceId,
    pub account_id: AccountId,
    pub display_name: String,
}

impl Machine {
    pub fn new(
        id: DeviceId,
        account_id: AccountId,
        display_name: impl Into<String>,
    ) -> Result<Self, PlatformError> {
        let display_name = display_name.into();
        if display_name.trim().is_empty() {
            return Err(PlatformError::EmptyName {
                kind: "machine display name",
            });
        }

        Ok(Self {
            id,
            account_id,
            display_name,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SharedFolderRole {
    Owner,
    Editor,
    Viewer,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SharedFolderMembership {
    pub account_id: AccountId,
    pub shared_folder_id: SharedFolderId,
    pub role: SharedFolderRole,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostedSharedFolder {
    pub id: SharedFolderId,
    pub owner_account_id: AccountId,
    pub display_name: String,
}

impl HostedSharedFolder {
    pub fn new(
        id: SharedFolderId,
        owner_account_id: AccountId,
        display_name: impl Into<String>,
    ) -> Result<Self, PlatformError> {
        let display_name = display_name.into();
        if display_name.trim().is_empty() {
            return Err(PlatformError::EmptyName {
                kind: "shared folder display name",
            });
        }

        Ok(Self {
            id,
            owner_account_id,
            display_name,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_memberships_reference_loom_shared_folders() {
        let account_id = AccountId::new("account-1").expect("account id");
        let shared_folder_id = SharedFolderId::new("folder-1").expect("folder id");
        let membership = SharedFolderMembership {
            account_id: account_id.clone(),
            shared_folder_id: shared_folder_id.clone(),
            role: SharedFolderRole::Owner,
        };
        let folder =
            HostedSharedFolder::new(shared_folder_id, account_id, "Bindhub").expect("folder");

        assert_eq!(membership.role, SharedFolderRole::Owner);
        assert_eq!(folder.display_name, "Bindhub");
    }
}
