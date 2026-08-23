//! Screen capture (Windows.Graphics.Capture) and hardware H.264 encode.
//!
//! [`wgc`] is the capture half — what there is to share, and the frames from
//! sharing it. [`encoder`] is the other half, and takes [`wgc::Frame`]'s
//! texture where it already is rather than a copy of it. [`share`] is what the
//! rest of the client uses: it owns one of each on a thread of their own and
//! hands back H.264 packets, so nothing outside this module holds a COM
//! interface.
//!
//! All three are Windows-only in the way the whole feature is: there is no
//! cross-platform seam here because there is no second platform, and a stub
//! that compiled elsewhere would only hide that.

#[cfg(windows)]
pub mod encoder;
#[cfg(windows)]
pub mod share;
#[cfg(windows)]
pub mod wgc;

use thiserror::Error;

/// Failures on the screen-share path.
#[derive(Debug, Error)]
pub enum CaptureError {
    /// No hardware H.264 encoder (NVENC / AMF / `QuickSync`) was found; the
    /// caller must warn the user before falling back to software.
    #[error("no hardware H.264 encoder available")]
    NoHardwareEncoder,
    /// Windows.Graphics.Capture is not available on this machine — Windows 10
    /// before 1903, or policy has turned it off.
    #[error("screen capture is not supported on this machine")]
    Unsupported,
    /// Listing monitors or windows failed.
    #[error("listing capture targets: {0}")]
    Enumerate(String),
    /// The capture could not be started: no device, no capture item, no pool.
    #[error("starting capture: {0}")]
    Start(String),
    /// A frame arrived but could not be read.
    #[error("reading a captured frame: {0}")]
    Frame(String),
    /// The capture session has ended — the window closed, or the display went
    /// away.
    #[error("the capture session has stopped")]
    Stopped,
    /// Media Foundation, the H.264 transform, or the colour conversion in
    /// front of it.
    #[error("encoding: {0}")]
    Encoder(String),
}
