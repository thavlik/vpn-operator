/// User-friendly message to display in `status.message` whenever a resource
/// is in the `Pending` phase.
pub const PENDING: &str = "Resource first appeared to the controller.";

/// User-friendly message to display in `status.message` whenever a resource's
/// deletion is pending garbage collection.
pub const TERMINATING: &str = "Resource deletion is pending garbage collection.";

/// User-friendly message to display in `status.message` whenever a `Mask`
/// or `MaskConsumer` is in the `Waiting` phase.
pub const WAITING: &str = "Waiting on a slot from a MaskProvider.";

/// User-friendly message to display in `status.message` whenever a `Mask`
/// or `MaskConsumer` is in the `Active` phase.
pub const ACTIVE: &str = "Reserving slot with the assigned MaskProvider.";

/// User-friendly message to display in `status.message` whenever a `Mask`
/// or `MaskConsumer` is in the `ErrNoProviders` phase.
pub const ERR_NO_PROVIDERS: &str = "No valid MaskProviders available.";
