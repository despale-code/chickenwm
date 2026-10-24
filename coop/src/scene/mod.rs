//! Phase 3.3, 3.4 & 4.1: 2D/3D Scene graph, Spring-damper physics (`F = -k*x - c*v`),
//! 16x16 deformable vertex meshes, live interactive resize with 60Hz Win32 throttling,
//! lazy hit-box alignment, and physical window management.

pub mod hierarchy;

use std::time::{Duration, Instant};
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{
    SetWindowPos, HWND_BOTTOM, SWP_NOACTIVATE, SWP_NOZORDER,
};

/// A 2D point/vector in the compositor's coordinate system.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Vec2 {
    pub x: f32,
    pub y: f32,
}

impl Vec2 {
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
}

/// Vertex attributes for the 16x16 deformable window mesh.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct QuadVertex {
    pub pos: [f32; 2],
    pub uv: [f32; 2],
}

/// Spring-damper physics state for fluid window animations (Hooke's Law: `F = -k*x - c*v`).
#[derive(Debug, Clone, Copy)]
pub struct SpringPhysics {
    pub position: Vec2,
    pub target: Vec2,
    pub velocity: Vec2,
    pub stiffness: f32,
    pub damping: f32,
    pub speed: f32,
}

impl SpringPhysics {
    pub fn new(initial: Vec2) -> Self {
        Self {
            position: initial,
            target: initial,
            velocity: Vec2::default(),
            stiffness: 520.0,
            damping: 36.0,
            speed: 2.2,
        }
    }

    /// Advances the simulation by `dt` seconds using semi-implicit Euler integration scaled by `speed`.
    pub fn step(&mut self, dt: f32) {
        let dt = (dt * self.speed).clamp(0.0, 0.05);
        let force_x =
            -self.stiffness * (self.position.x - self.target.x) - self.damping * self.velocity.x;
        let force_y =
            -self.stiffness * (self.position.y - self.target.y) - self.damping * self.velocity.y;

        self.velocity.x += force_x * dt;
        self.velocity.y += force_y * dt;

        self.position.x += self.velocity.x * dt;
        self.position.y += self.velocity.y * dt;

        if self.is_settled() {
            self.position = self.target;
            self.velocity = Vec2::default();
        }
    }

    pub fn is_settled(&self) -> bool {
        (self.position.x - self.target.x).abs() < 0.1
            && (self.position.y - self.target.y).abs() < 0.1
            && self.velocity.x.abs() < 0.1
            && self.velocity.y.abs() < 0.1
    }
}

/// Represents a managed window quad on the GPU canvas.
pub struct WindowQuad {
    pub hwnd: HWND,
    pub physics: SpringPhysics,
    pub size_physics: SpringPhysics,
    pub width: f32,
    pub height: f32,
    pub tex_width: u32,
    pub tex_height: u32,
    pub z_order: i32,
    pub workspace_id: u32,
    pub hit_box_dirty: bool,
    pub last_win32_resize: Instant,
}

impl WindowQuad {
    pub const MESH_SUBDIVISIONS: usize = 16;
    pub const WIN32_RESIZE_INTERVAL: Duration = Duration::from_millis(16); // ~60Hz throttle

    pub fn new(hwnd: HWND, pos: Vec2, width: f32, height: f32) -> Self {
        let size = Vec2::new(width, height);
        Self {
            hwnd,
            physics: SpringPhysics::new(pos),
            size_physics: SpringPhysics::new(size),
            width,
            height,
            tex_width: width.max(1.0) as u32,
            tex_height: height.max(1.0) as u32,
            z_order: 0,
            workspace_id: 1,
            hit_box_dirty: false,
            last_win32_resize: Instant::now(),
        }
    }

    /// Sets the master animation speed multiplier for position and size springs.
    pub fn set_speed(&mut self, speed: f32) {
        self.physics.speed = speed;
        self.size_physics.speed = speed;
    }

    /// Sets a new animated target position and marks the Win32 physical hit-box for lazy sync on rest.
    pub fn set_target_position(&mut self, target: Vec2) {
        self.physics.target = target;
        self.hit_box_dirty = true;
    }

    /// Phase 3.4: Updates target size during live interactive resize, stretching the visual quad
    /// immediately at 240Hz+ while throttling Win32 `SetWindowPos` updates to 60Hz.
    pub fn live_resize(&mut self, target_w: f32, target_h: f32) {
        self.size_physics.target = Vec2::new(target_w.max(64.0), target_h.max(64.0));
        self.hit_box_dirty = true;

        if self.last_win32_resize.elapsed() >= Self::WIN32_RESIZE_INTERVAL && !self.hwnd.0.is_null()
        {
            self.last_win32_resize = Instant::now();
            unsafe {
                let _ = SetWindowPos(
                    self.hwnd,
                    HWND_BOTTOM,
                    self.physics.target.x.round() as i32,
                    self.physics.target.y.round() as i32,
                    self.size_physics.target.x.round() as i32,
                    self.size_physics.target.y.round() as i32,
                    SWP_NOACTIVATE | SWP_NOZORDER,
                );
            }
        }
    }

    /// Advances position and size spring physics by `dt` seconds, and performs lazy Win32 hit-box
    /// alignment once the spring settles (Phase 3.4).
    pub fn step(&mut self, dt: f32) {
        self.physics.step(dt);
        self.size_physics.step(dt);
        self.width = self.size_physics.position.x;
        self.height = self.size_physics.position.y;

        if self.hit_box_dirty && self.physics.is_settled() && self.size_physics.is_settled() {
            self.sync_physical_hitbox();
            self.hit_box_dirty = false;
        }
    }

    /// Phase 3.4 & 4.1: Aligns the real Win32 window silently behind the Vulkan canvas (`HWND_BOTTOM`)
    /// so OS hit-testing and relative child coordinates match the resting visual quad.
    pub fn sync_physical_hitbox(&self) {
        if self.hwnd.0.is_null() {
            return;
        }
        unsafe {
            let _ = SetWindowPos(
                self.hwnd,
                HWND_BOTTOM,
                self.physics.target.x.round() as i32,
                self.physics.target.y.round() as i32,
                self.size_physics.target.x.round() as i32,
                self.size_physics.target.y.round() as i32,
                SWP_NOACTIVATE,
            );
        }
    }

    /// Phase 4.1: Strips native Win32 caption/borders and places the real window behind the Vulkan canvas.
    pub fn prepare_physical_window(&self) {
        if self.hwnd.0.is_null() {
            return;
        }
        let _ = chickenwm_core::sys::attach_in_place_hit_test(self.hwnd);
        self.sync_physical_hitbox();
    }

    /// Phase 3.4: Computes UV scaling factors when the visual quad is stretched ahead of the WGC texture.
    pub fn uv_scale(&self) -> [f32; 2] {
        let tw = (self.tex_width.max(1)) as f32;
        let th = (self.tex_height.max(1)) as f32;
        [(self.width / tw).clamp(0.25, 4.0), (self.height / th).clamp(0.25, 4.0)]
    }

    /// Phase 3.3: Generates a 16x16 subdivided vertex mesh (`17x17` vertices, `1536` indices)
    /// with non-rigid wobbly/jelly vertex displacement driven by spring velocity.
    pub fn generate_deformable_mesh(&self) -> (Vec<QuadVertex>, Vec<u16>) {
        let n = Self::MESH_SUBDIVISIONS;
        let mut vertices = Vec::with_capacity((n + 1) * (n + 1));
        let mut indices = Vec::with_capacity(n * n * 6);

        let vel_x = self.physics.velocity.x;
        let vel_y = self.physics.velocity.y;
        let jelly_factor = 0.018f32;

        for row in 0..=n {
            let v = row as f32 / n as f32;
            for col in 0..=n {
                let u = col as f32 / n as f32;

                // Non-rigid lag increases toward bottom/trailing edges
                let weight = (u * (1.0 - u) + v * 0.75).clamp(0.0, 1.0);
                let disp_x = -vel_x * weight * jelly_factor;
                let disp_y = -vel_y * weight * jelly_factor;

                let px = self.physics.position.x + u * self.width + disp_x;
                let py = self.physics.position.y + v * self.height + disp_y;

                vertices.push(QuadVertex {
                    pos: [px, py],
                    uv: [u, v],
                });
            }
        }

        let stride = (n + 1) as u16;
        for row in 0..(n as u16) {
            for col in 0..(n as u16) {
                let tl = row * stride + col;
                let tr = tl + 1;
                let bl = (row + 1) * stride + col;
                let br = bl + 1;
                indices.extend_from_slice(&[tl, bl, tr, tr, bl, br]);
            }
        }

        (vertices, indices)
    }

    /// Returns `true` if `(px, py)` lies within this window quad's visual bounds.
    pub fn hit_test(&self, px: f32, py: f32) -> bool {
        px >= self.physics.position.x
            && px <= self.physics.position.x + self.width
            && py >= self.physics.position.y
            && py <= self.physics.position.y + self.height
    }
}
