//! `coop`: The Pure User-Mode Vulkan Viewspace Compositor for ChickenWM.
//!
//! Replaces DWM's presentation layer for desktop productivity windows.
//! Captures Win32 windows into zero-copy DXGI shared textures, maps them to Vulkan VkImages,
//! and renders them onto a freeform GPU canvas with spring-damper physics and custom shaders.

pub mod capture;
pub mod input;
pub mod power;
pub mod scene;
pub mod sync;
pub mod vulkan;
