//! Codec registry — discover available encoders/decoders by id or name.

use rumpeg_util::{CodecId, MediaType};
use std::sync::{Arc, OnceLock, RwLock};

/// Whether an entry describes an encoder, decoder, or both.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodecKind {
    /// Decoder only.
    Decoder,
    /// Encoder only.
    Encoder,
    /// Both directions available.
    Both,
}

/// Metadata about a registered codec implementation.
#[derive(Clone, Debug)]
pub struct CodecDescriptor {
    /// Codec id.
    pub id: CodecId,
    /// Canonical short name.
    pub name: &'static str,
    /// Long descriptive name.
    pub long_name: &'static str,
    /// Media type.
    pub media_type: MediaType,
    /// Encoder / decoder availability.
    pub kind: CodecKind,
}

/// Process-wide codec registry.
#[derive(Debug, Default)]
pub struct Registry {
    codecs: RwLock<Vec<CodecDescriptor>>,
}

impl Registry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register built-in PCM and rawvideo codecs.
    pub fn register_builtins(&self) {
        let builtins = [
            CodecDescriptor {
                id: CodecId::PcmS16Le,
                name: "pcm_s16le",
                long_name: "PCM signed 16-bit little-endian",
                media_type: MediaType::Audio,
                kind: CodecKind::Both,
            },
            CodecDescriptor {
                id: CodecId::PcmS24Le,
                name: "pcm_s24le",
                long_name: "PCM signed 24-bit little-endian",
                media_type: MediaType::Audio,
                kind: CodecKind::Both,
            },
            CodecDescriptor {
                id: CodecId::PcmS32Le,
                name: "pcm_s32le",
                long_name: "PCM signed 32-bit little-endian",
                media_type: MediaType::Audio,
                kind: CodecKind::Both,
            },
            CodecDescriptor {
                id: CodecId::PcmF32Le,
                name: "pcm_f32le",
                long_name: "PCM 32-bit floating point little-endian",
                media_type: MediaType::Audio,
                kind: CodecKind::Both,
            },
            CodecDescriptor {
                id: CodecId::PcmU8,
                name: "pcm_u8",
                long_name: "PCM unsigned 8-bit",
                media_type: MediaType::Audio,
                kind: CodecKind::Both,
            },
            CodecDescriptor {
                id: CodecId::RawVideo,
                name: "rawvideo",
                long_name: "raw video",
                media_type: MediaType::Video,
                kind: CodecKind::Both,
            },
            // Placeholders for future implementations — discoverable in listings.
            CodecDescriptor {
                id: CodecId::Flac,
                name: "flac",
                long_name: "FLAC (Free Lossless Audio Codec)",
                media_type: MediaType::Audio,
                kind: CodecKind::Both,
            },
            CodecDescriptor {
                id: CodecId::H264,
                name: "h264",
                long_name: "H.264 / AVC (pure-Rust I_PCM Baseline)",
                media_type: MediaType::Video,
                kind: CodecKind::Both,
            },
            CodecDescriptor {
                id: CodecId::Mjpeg,
                name: "mjpeg",
                long_name: "Motion JPEG / JPEG image",
                media_type: MediaType::Video,
                kind: CodecKind::Encoder,
            },
            CodecDescriptor {
                id: CodecId::Av1,
                name: "av1",
                long_name: "Alliance for Open Media AV1",
                media_type: MediaType::Video,
                kind: CodecKind::Decoder,
            },
        ];
        let mut guard = self.codecs.write().expect("registry lock");
        for desc in builtins {
            if !guard.iter().any(|c| c.id == desc.id) {
                guard.push(desc);
            }
        }
    }

    /// Register a custom codec descriptor.
    pub fn register(&self, desc: CodecDescriptor) {
        let mut guard = self.codecs.write().expect("registry lock");
        if let Some(existing) = guard.iter_mut().find(|c| c.id == desc.id) {
            *existing = desc;
        } else {
            guard.push(desc);
        }
    }

    /// List all registered codecs.
    pub fn list(&self) -> Vec<CodecDescriptor> {
        self.codecs.read().expect("registry lock").clone()
    }

    /// Find by codec id.
    pub fn find(&self, id: CodecId) -> Option<CodecDescriptor> {
        self.codecs
            .read()
            .expect("registry lock")
            .iter()
            .find(|c| c.id == id)
            .cloned()
    }

    /// Find by name.
    pub fn find_by_name(&self, name: &str) -> Option<CodecDescriptor> {
        self.codecs
            .read()
            .expect("registry lock")
            .iter()
            .find(|c| c.name == name)
            .cloned()
    }

    /// Returns `Some` if a decoder entry exists (may still be unimplemented).
    pub fn find_decoder(&self, id: CodecId) -> Option<CodecDescriptor> {
        self.find(id)
            .filter(|d| matches!(d.kind, CodecKind::Decoder | CodecKind::Both))
    }

    /// Returns `Some` if an encoder entry exists.
    pub fn find_encoder(&self, id: CodecId) -> Option<CodecDescriptor> {
        self.find(id)
            .filter(|d| matches!(d.kind, CodecKind::Encoder | CodecKind::Both))
    }
}

static GLOBAL: OnceLock<Arc<Registry>> = OnceLock::new();

/// Process-global codec registry.
pub fn global() -> Arc<Registry> {
    GLOBAL
        .get_or_init(|| {
            let reg = Arc::new(Registry::new());
            reg.register_builtins();
            reg
        })
        .clone()
}
