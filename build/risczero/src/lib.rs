#[cfg(feature = "rebuild-fpvm")]
include!(concat!(env!("OUT_DIR"), "/methods.rs"));

#[cfg(not(feature = "rebuild-fpvm"))]
include!("./methods.rs");
