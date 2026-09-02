//! On-screen UI modes: boot banner, analog clock, in-memory `.qgif` playback,
//! and the SD face player. Each is a small, self-contained display routine.

pub mod boot;
pub mod clock;
pub mod face_player;
pub mod qgif;
