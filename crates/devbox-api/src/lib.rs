//! Devbox hosted API boundary.
//!
//! This crate is a compileable skeleton for the hosted API work that will fill
//! in PR6. It intentionally depends on platform and Loom vocabulary instead of
//! owning folder-state semantics itself.

use devbox_platform::{AccountId, DeviceId, SharedFolderRole};
use loom_core::{CursorId, FolderRevisionId, SharedFolderId};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApiArea {
    Auth,
    Devices,
    SharedFolders,
    LoomRemote,
    ObjectAccess,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiRoute {
    pub area: ApiArea,
    pub path: &'static str,
}

impl ApiRoute {
    pub const fn new(area: ApiArea, path: &'static str) -> Self {
        Self { area, path }
    }
}

pub const API_ROUTES: &[ApiRoute] = &[
    ApiRoute::new(ApiArea::Auth, "/v1/auth"),
    ApiRoute::new(ApiArea::Devices, "/v1/devices"),
    ApiRoute::new(ApiArea::SharedFolders, "/v1/shared-folders"),
    ApiRoute::new(ApiArea::LoomRemote, "/v1/loom"),
    ApiRoute::new(ApiArea::ObjectAccess, "/v1/object-access"),
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SharedFolderResponse {
    pub id: SharedFolderId,
    pub account_id: AccountId,
    pub role: SharedFolderRole,
    pub display_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceResponse {
    pub id: DeviceId,
    pub account_id: AccountId,
    pub display_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CursorUpdateRequest {
    pub shared_folder_id: SharedFolderId,
    pub cursor_id: CursorId,
    pub expected_revision_id: Option<FolderRevisionId>,
    pub next_revision_id: FolderRevisionId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ApiBoundary {
    pub owns_accounts: bool,
    pub owns_devices: bool,
    pub owns_shared_folder_membership: bool,
    pub owns_folder_state_semantics: bool,
}

impl ApiBoundary {
    pub fn devbox_hosted_api() -> Self {
        Self {
            owns_accounts: true,
            owns_devices: true,
            owns_shared_folder_membership: true,
            owns_folder_state_semantics: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_boundary_delegates_folder_state_to_loom() {
        let boundary = ApiBoundary::devbox_hosted_api();

        assert!(boundary.owns_accounts);
        assert!(boundary.owns_devices);
        assert!(boundary.owns_shared_folder_membership);
        assert!(!boundary.owns_folder_state_semantics);
    }

    #[test]
    fn api_routes_include_loom_remote_without_git_route_names() {
        let paths = API_ROUTES
            .iter()
            .map(|route| route.path)
            .collect::<Vec<_>>();

        assert!(paths.contains(&"/v1/loom"));
        assert!(!paths.iter().any(|path| path.contains("git")));
    }
}
