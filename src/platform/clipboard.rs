//! Host clipboard conventions used for Rozi's default interaction policy.

/// Linux desktops conventionally expose both a regular clipboard and a primary selection.
pub const fn has_primary_selection_convention() -> bool {
    cfg!(target_os = "linux")
}
