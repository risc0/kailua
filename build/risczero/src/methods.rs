// todo: load actual ELFs and absolute paths
// todo: auto compute image ids

pub const KAILUA_FPVM_KONA_ELF: &[u8] = include_bytes!("kailua-fpvm-kona.bin");
pub const KAILUA_FPVM_KONA_PATH: &str = "./kailua-fpvm-kona.bin";
pub const KAILUA_FPVM_KONA_ID: [u32; 8] = [1077155897, 1736134679, 887303435, 1953326413, 749254628, 3334869102, 2938415251, 617388085];

pub const KAILUA_FPVM_HOKULEA_ELF: &[u8] = include_bytes!("kailua-fpvm-kona.bin");
pub const KAILUA_FPVM_HOKULEA_PATH: &str = "./kailua-fpvm-kona.bin";
pub const KAILUA_FPVM_HOKULEA_ID: [u32; 8] = [1077155897, 1736134679, 887303435, 1953326413, 749254628, 3334869102, 2938415251, 617388085];

pub const KAILUA_DA_HOKULEA_ELF: &[u8] = include_bytes!("kailua-fpvm-kona.bin");
pub const KAILUA_DA_HOKULEA_PATH: &str = "./kailua-fpvm-kona.bin";
pub const KAILUA_DA_HOKULEA_ID: [u32; 8] = [1077155897, 1736134679, 887303435, 1953326413, 749254628, 3334869102, 2938415251, 617388085];
