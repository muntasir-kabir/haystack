//! Presentation registry for embedded-data detectors. Detector grammar lives
//! in core; each format gets a small, independently tested UI profile here.

pub mod highlighters;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Presentation {
    pub badge: &'static str,
    pub title: &'static str,
    pub primary_tab: &'static str,
    pub explicit_decode: bool,
}

pub fn presentation(detector_id: &str) -> Presentation {
    highlighters::for_detector(detector_id)
}
