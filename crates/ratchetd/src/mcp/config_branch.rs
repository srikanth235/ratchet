//! Where a drafted optimization's config goes: the `ratchet/config` branch of the repo, and nowhere else.
//!
//! The first draft creates the branch from the repo's base ref (main plus one commit with the config files); later drafts add a
//! commit on top of it. Nothing is ever committed to the base ref itself, and no pull request is opened.

use ratchet_core::ports::{CommitFile, CommitFilesInput, CommitRef, GitHost, PortResult};

/// The only branch the MCP server writes to.
pub const CONFIG_BRANCH: &str = "ratchet/config";

/// Commit `files` to [`CONFIG_BRANCH`] of `host`. `base_ref` is where the branch starts when it does not exist yet.
pub async fn commit_config(
    host: &dyn GitHost,
    base_ref: &str,
    message: &str,
    files: Vec<CommitFile>,
) -> PortResult<CommitRef> {
    let exists = host.resolve_ref(CONFIG_BRANCH).await.is_ok();
    let input = CommitFilesInput {
        base_ref: if exists { CONFIG_BRANCH } else { base_ref }.to_owned(),
        branch: CONFIG_BRANCH.to_owned(),
        message: message.to_owned(),
        files,
        update: exists,
    };
    debug_assert_eq!(input.branch, CONFIG_BRANCH);
    host.commit_files(&input).await
}
