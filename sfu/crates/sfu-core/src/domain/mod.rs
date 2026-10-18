//! Pure domain rules of the SFU. Nothing in here performs IO or depends on str0m.

pub mod bitrate_meter;
pub mod codec;
pub mod keyframe;
pub mod layer_allocation;
pub mod layer_selector;
pub mod opus;
pub mod room;
pub mod rtp_rewriter;
pub mod session;
pub mod sink_wants;
pub mod speaking;
pub mod svc_filter;
pub mod vp9_descriptor;
