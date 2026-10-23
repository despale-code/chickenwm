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

#[cfg(test)]
mod tests {
    use crate::input::{hit_test_quads, AtomicCursor};
    use crate::scene::{SpringPhysics, Vec2, WindowQuad};
    use crate::vulkan::pipeline::{
        catmull_rom_weights, premultiplied_alpha_blend_attachment, ExposeOverview,
    };
    use ash::vk;
    use windows::Win32::Foundation::HWND;

    #[test]
    fn test_spring_physics_convergence() {
        let mut spring = SpringPhysics::new(Vec2::new(0.0, 0.0));
        spring.target = Vec2::new(500.0, 300.0);
        assert!(!spring.is_settled());

        for _ in 0..300 {
            spring.step(1.0 / 120.0);
        }
        assert!(spring.is_settled());
        assert!((spring.position.x - 500.0).abs() < 0.01);
        assert!((spring.position.y - 300.0).abs() < 0.01);
    }

    #[test]
    fn test_deformable_16x16_mesh_generation() {
        let mut quad = WindowQuad::new(HWND::default(), Vec2::new(100.0, 100.0), 800.0, 600.0);
        quad.physics.velocity = Vec2::new(250.0, -120.0);
        let (verts, indices) = quad.generate_deformable_mesh();
        assert_eq!(verts.len(), 17 * 17);
        assert_eq!(indices.len(), 16 * 16 * 6);
    }

    #[test]
    fn test_catmull_rom_partition_of_unity() {
        for i in 0..=10 {
            let t = i as f32 / 10.0;
            let w = catmull_rom_weights(t);
            let sum: f32 = w.iter().sum();
            assert!((sum - 1.0).abs() < 1e-5);
        }
    }

    #[test]
    fn test_premultiplied_alpha_blend_state() {
        let blend = premultiplied_alpha_blend_attachment();
        assert_eq!(blend.blend_enable, vk::TRUE);
        assert_eq!(blend.src_color_blend_factor, vk::BlendFactor::ONE);
        assert_eq!(
            blend.dst_color_blend_factor,
            vk::BlendFactor::ONE_MINUS_SRC_ALPHA
        );
        assert_eq!(blend.src_alpha_blend_factor, vk::BlendFactor::ONE);
        assert_eq!(
            blend.dst_alpha_blend_factor,
            vk::BlendFactor::ONE_MINUS_SRC_ALPHA
        );
    }

    #[test]
    fn test_atomic_cursor_vsync_decoupling() {
        let cursor = AtomicCursor::new();
        assert_eq!(cursor.consume_for_vsync(), None);
        cursor.update(1200, 800);
        cursor.update(1205, 810);
        assert_eq!(cursor.consume_for_vsync(), Some((1205, 810)));
        assert_eq!(cursor.consume_for_vsync(), None);
    }

    #[test]
    fn test_quad_hittest_z_order() {
        let mut q1 = WindowQuad::new(HWND(1 as *mut _), Vec2::new(0.0, 0.0), 400.0, 400.0);
        q1.z_order = 1;
        let mut q2 = WindowQuad::new(HWND(2 as *mut _), Vec2::new(100.0, 100.0), 400.0, 400.0);
        q2.z_order = 5;
        let quads = [q1, q2];

        let hit = hit_test_quads(&quads, 150.0, 150.0).unwrap();
        assert_eq!(hit.hwnd.0 as usize, 2);
    }

    #[test]
    fn test_expose_grid_slots() {
        let expose = ExposeOverview::new();
        let (x0, y0, w0, h0) = expose.compute_slot(0, 4, 1920.0, 1080.0);
        let (x3, y3, _, _) = expose.compute_slot(3, 4, 1920.0, 1080.0);
        assert!(w0 > 800.0 && h0 > 400.0);
        assert!(x3 > x0 && y3 > y0);
    }

    #[test]
    fn test_live_d3d11_to_vulkan_nt_handle_import() {
        let cdev = crate::capture::CaptureDevice::new().expect("CaptureDevice::new failed");
        let (_tex, handle) = cdev
            .create_shared_texture(256, 256)
            .expect("create_shared_texture failed");
        let vk_ctx = crate::vulkan::VulkanContext::new().expect("VulkanContext::new failed");
        let mut imported = vk_ctx
            .import_dxgi_texture(handle, 256, 256)
            .expect("import_dxgi_texture failed");
        unsafe {
            imported.destroy(&vk_ctx.device);
            let _ = windows::Win32::Foundation::CloseHandle(handle);
        }
    }
}
