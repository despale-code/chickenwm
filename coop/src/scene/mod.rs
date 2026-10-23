//! 2D/3D Scene graph, Spring-damper physics, and window quad geometry.

pub mod hierarchy;

use windows::Win32::Foundation::HWND;

/// A 2D point/vector in the compositor's coordinate system.
#[derive(Debug, Clone, Copy, Default)]
pub struct Vec2 {
    pub x: f32,
    pub y: f32,
}

/// Spring-damper physics state for fluid window animations (Hooke's Law: F = -k*x - c*v).
#[derive(Debug, Clone, Copy)]
pub struct SpringPhysics {
    pub position: Vec2,
    pub target: Vec2,
    pub velocity: Vec2,
    pub stiffness: f32,
    pub damping: f32,
}

impl SpringPhysics {
    pub fn new(initial: Vec2) -> Self {
        Self {
            position: initial,
            target: initial,
            velocity: Vec2::default(),
            stiffness: 170.0,
            damping: 26.0,
        }
    }

    /// Advances the simulation by `dt` seconds.
    pub fn step(&mut self, dt: f32) {
        let force_x = -self.stiffness * (self.position.x - self.target.x) - self.damping * self.velocity.x;
        let force_y = -self.stiffness * (self.position.y - self.target.y) - self.damping * self.velocity.y;

        self.velocity.x += force_x * dt;
        self.velocity.y += force_y * dt;

        self.position.x += self.velocity.x * dt;
        self.position.y += self.velocity.y * dt;
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
    pub width: f32,
    pub height: f32,
    pub z_order: i32,
}

impl WindowQuad {
    pub fn new(hwnd: HWND, pos: Vec2, width: f32, height: f32) -> Self {
        Self {
            hwnd,
            physics: SpringPhysics::new(pos),
            width,
            height,
            z_order: 0,
        }
    }
}
