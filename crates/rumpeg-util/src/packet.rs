//! Compressed media packets (`AVPacket` counterpart).

use crate::buffer::Buffer;
use crate::time::Timestamp;

/// A compressed packet of elementary stream data.
#[derive(Clone, Debug, Default)]
pub struct Packet {
    /// Compressed payload (shared via [`Buffer`]).
    pub data: Buffer,
    /// Presentation timestamp in the stream time base.
    pub pts: Timestamp,
    /// Decode timestamp in the stream time base.
    pub dts: Timestamp,
    /// Duration in stream time-base ticks (0 if unknown).
    pub duration: i64,
    /// Index of the stream this packet belongs to.
    pub stream_index: usize,
    /// Packet flags.
    pub flags: PacketFlags,
    /// Byte position in the source file, if known (`-1` = unknown).
    pub pos: i64,
}

bitflags_shim::packet_flags! {
    /// Bit flags for [`Packet`].
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    pub struct PacketFlags: u32 {
        /// Keyframe / sync sample.
        const KEY = 1 << 0;
        /// Corrupt data.
        const CORRUPT = 1 << 1;
        /// Discardable packet.
        const DISCARD = 1 << 2;
    }
}

// Lightweight bitflags without an extra dependency.
mod bitflags_shim {
    macro_rules! packet_flags {
        (
            $(#[$meta:meta])*
            pub struct $name:ident: $ty:ty {
                $(
                    $(#[$flag_meta:meta])*
                    const $flag:ident = $value:expr;
                )*
            }
        ) => {
            $(#[$meta])*
            pub struct $name(pub $ty);

            impl $name {
                $(
                    $(#[$flag_meta])*
                    pub const $flag: Self = Self($value);
                )*

                /// Empty flag set.
                pub const fn empty() -> Self {
                    Self(0)
                }

                /// Returns `true` if all bits in `other` are set.
                pub const fn contains(self, other: Self) -> bool {
                    self.0 & other.0 == other.0
                }

                /// Insert bits.
                pub fn insert(&mut self, other: Self) {
                    self.0 |= other.0;
                }

                /// Returns `true` if no bits are set.
                pub const fn is_empty(self) -> bool {
                    self.0 == 0
                }
            }

            impl Default for $name {
                fn default() -> Self {
                    Self::empty()
                }
            }

            impl std::ops::BitOr for $name {
                type Output = Self;
                fn bitor(self, rhs: Self) -> Self {
                    Self(self.0 | rhs.0)
                }
            }
        };
    }
    pub(crate) use packet_flags;
}

impl Packet {
    /// Create a packet from owned bytes.
    pub fn new(data: impl Into<Buffer>) -> Self {
        Self {
            data: data.into(),
            pts: Timestamp::NONE,
            dts: Timestamp::NONE,
            duration: 0,
            stream_index: 0,
            flags: PacketFlags::empty(),
            pos: -1,
        }
    }

    /// Payload length in bytes.
    #[inline]
    pub fn size(&self) -> usize {
        self.data.len()
    }

    /// `true` if this packet is marked as a keyframe.
    pub fn is_key(&self) -> bool {
        self.flags.contains(PacketFlags::KEY)
    }
}
