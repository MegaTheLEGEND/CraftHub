//! The product's name in one place, so renaming it is a one-file change.
//!
//! The look follows the ArtCraft Launcher; the name is CraftHub. See NOTICE.

/// Shown in the window title and the header wordmark.
pub const PRODUCT: &str = "CraftHub";
/// Left half of the header wordmark (set in the display face).
pub const WORDMARK: &str = "CRAFTHUB";
/// The tag next to the wordmark (the launcher says "Launcher" here).
pub const TAG: &str = "Self-hosted";
/// Sent by the UI on every state-changing call (CSRF guard).
pub const REQUESTED_WITH: &str = "crafthub";
/// Where this project lives.
pub const REPO_URL: &str = "https://github.com/MegaTheLEGEND/CraftHub";
