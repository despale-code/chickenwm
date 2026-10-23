//! `chickenscope`: The Low-Latency Game Mode & In-Game Overlay Engine for ChickenWM.
//!
//! Automatically detects games, steps coop aside into 0% GPU sleep, grants native DirectFlip
//! display passthrough, boosts MMCSS process priority, and provides an in-game HUD overlay.

pub mod detect;
pub mod overlay;
pub mod passthrough;
