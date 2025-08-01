#[cfg(feature = "rebuild-fpvm")]
include!(concat!(env!("OUT_DIR"), "/methods.rs"));

#[cfg(feature = "rebuild-fpvm")]
pub use canoe_steel_methods::CERT_VERIFICATION_ELF as KAILUA_DA_HOKULEA_ELF;
#[cfg(feature = "rebuild-fpvm")]
pub use canoe_steel_methods::CERT_VERIFICATION_ID as KAILUA_DA_HOKULEA_ID;
#[cfg(feature = "rebuild-fpvm")]
pub use canoe_steel_methods::CERT_VERIFICATION_PATH as KAILUA_DA_HOKULEA_PATH;

#[cfg(not(feature = "rebuild-fpvm"))]
include!("./methods.rs");
