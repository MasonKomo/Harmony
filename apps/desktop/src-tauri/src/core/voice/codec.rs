#![allow(dead_code)]

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodecCapabilities {
    pub opus_backend: &'static str,
}

impl Default for CodecCapabilities {
    fn default() -> Self {
        Self {
            opus_backend: opus_backend_name(),
        }
    }
}

pub fn opus_backend_name() -> &'static str {
    #[cfg(feature = "real-opus")]
    {
        return "opus2";
    }

    #[cfg(not(feature = "real-opus"))]
    {
        "disabled"
    }
}
