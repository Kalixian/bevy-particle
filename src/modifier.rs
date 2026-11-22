use bevy::{
    prelude::*,
    render::{extract_resource::ExtractResource, render_resource::ShaderType},
    window::PrimaryWindow,
};

/// Shared modifier plumbing. Which particles a modifier affects depends on where the
/// [`Modifiers`] (or [`MouseAttractor`]) component sits:
/// - on a `ParticleSpace`: every particle in the space;
/// - on a `ParticleEmitter`: only that emitter's particles.
///
/// Each frame, space modifiers run first, then emitter modifiers, each list in order.
pub struct ModifierPlugin;

impl Plugin for ModifierPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ModifierUpload>()
            .init_resource::<MouseTarget>()
            .add_systems(Update, update_mouse_target)
            .add_systems(
                PostUpdate,
                clear_modifier_upload.in_set(ModifierSystems::Clear),
            );
    }
}

/// While the left mouse button is held, pulls particles toward the cursor like a
/// [`Modifier::PointAttractor`] there; does nothing while it's released. Put it on a
/// `ParticleSpace` to pull every particle, or on a `ParticleEmitter` to pull only its own.
///
/// It runs after the owner's [`Modifiers`] list and doesn't change it.
#[derive(Component, Debug, Clone, Copy)]
pub struct MouseAttractor {
    /// Acceleration = strength / distance², like `PointAttractor`. Negative repels.
    pub strength: f32,
    /// Upper limit on the acceleration.
    pub max_accel: f32,
}

/// The cursor's world position while the left mouse button is held, otherwise `None`.
#[derive(Resource, Default, Debug, Clone, Copy)]
pub struct MouseTarget(pub Option<Vec2>);

fn update_mouse_target(
    buttons: Res<ButtonInput<MouseButton>>,
    window: Single<&Window, With<PrimaryWindow>>,
    camera: Single<(&Camera, &GlobalTransform)>,
    mut target: ResMut<MouseTarget>,
) {
    let (camera, camera_transform) = *camera;
    target.0 = buttons
        .pressed(MouseButton::Left)
        .then(|| window.cursor_position())
        .flatten()
        .and_then(|cursor| camera.viewport_to_world_2d(camera_transform, cursor).ok());
}

/// Upload order: `Clear` runs before any system that calls [`ModifierUpload::push`].
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub enum ModifierSystems {
    Clear,
}

/// Behaviors applied to particles every frame. Add a variant here and a matching
/// `case` in `apply_modifier` in `particle_sim.wgsl`.
///
/// Positions (`offset`) are relative to the owning space or emitter, so attractors move
/// with their owner.
#[derive(Debug, Clone)]
#[allow(dead_code)] // A toolbox: the demo scene doesn't use every variant.
pub enum Modifier {
    /// Constant acceleration, e.g. wind or downward gravity.
    Acceleration(Vec2),
    /// Inverse-square pull toward a point: `strength / distance²`, capped at `max_accel`.
    /// Negative strength repels.
    PointAttractor {
        offset: Vec2,
        strength: f32,
        max_accel: f32,
    },
    /// Swirl around a point: tangential `strength / distance`, capped at `max_accel`.
    /// Positive is counter-clockwise.
    Vortex {
        offset: Vec2,
        strength: f32,
        max_accel: f32,
    },
    /// Exponential velocity damping, as a fraction lost per second (roughly).
    Drag(f32),
    /// Bounces particles off the edges of a rectangle centered on the owner.
    /// `restitution` is the fraction of speed kept on impact.
    Bounds {
        half_extents: Vec2,
        restitution: f32,
    },
    /// Sets color from start to end over each particle's life (replaces the spawn color).
    /// No effect on immortal particles.
    ColorOverLife { start: Color, end: Color },
    /// Sets radius from start to end over life. No effect on immortal particles.
    SizeOverLife { start: f32, end: f32 },
    /// Sets mass from start to end over life. No effect on immortal particles.
    MassOverLife { start: f32, end: f32 },
}

/// The modifier list of a space or emitter.
#[derive(Component, Debug, Clone, Default, Deref, DerefMut)]
pub struct Modifiers(pub Vec<Modifier>);

impl Modifiers {
    pub fn new(modifiers: impl IntoIterator<Item = Modifier>) -> Self {
        Self(modifiers.into_iter().collect())
    }
}

/// One modifier as seen by the GPU. Must match `struct Modifier` in `particle_sim.wgsl`.
/// The meaning of `a` and `b` depends on `kind`; see [`GpuModifier::encode`].
#[derive(ShaderType, Clone, Copy, Debug, Default)]
pub struct GpuModifier {
    pub kind: u32,
    pub a: Vec4,
    pub b: Vec4,
}

impl GpuModifier {
    /// `origin` is the owner's world position; offsets are resolved against it here.
    fn encode(modifier: &Modifier, origin: Vec2) -> Self {
        let (kind, a, b) = match *modifier {
            Modifier::Acceleration(accel) => (0, accel.extend(0.0).extend(0.0), Vec4::ZERO),
            Modifier::PointAttractor { offset, strength, max_accel } => {
                (1, (origin + offset).extend(strength).extend(max_accel), Vec4::ZERO)
            }
            Modifier::Vortex { offset, strength, max_accel } => {
                (2, (origin + offset).extend(strength).extend(max_accel), Vec4::ZERO)
            }
            Modifier::Drag(k) => (3, Vec4::new(k, 0.0, 0.0, 0.0), Vec4::ZERO),
            Modifier::Bounds { half_extents, restitution } => (
                4,
                origin.extend(half_extents.x).extend(half_extents.y),
                Vec4::new(restitution, 0.0, 0.0, 0.0),
            ),
            Modifier::ColorOverLife { start, end } => (
                5,
                Vec4::from_array(start.to_linear().to_f32_array()),
                Vec4::from_array(end.to_linear().to_f32_array()),
            ),
            Modifier::SizeOverLife { start, end } => (6, Vec4::new(start, end, 0.0, 0.0), Vec4::ZERO),
            Modifier::MassOverLife { start, end } => (7, Vec4::new(start, end, 0.0, 0.0), Vec4::ZERO),
        };
        Self { kind, a, b }
    }
}

/// Every space and emitter modifier for this frame, in one array. Owners refer to their
/// slice of it by `(offset, count)`.
#[derive(Resource, Clone, Default, ExtractResource)]
#[extract_app(bevy::render::RenderApp)]
pub struct ModifierUpload {
    modifiers: Vec<GpuModifier>,
    /// This frame's [`MouseTarget`], copied in when the upload is cleared.
    mouse_target: Option<Vec2>,
}

impl ModifierUpload {
    /// Appends an owner's `modifiers`, then its [`MouseAttractor`] if the mouse is held, and
    /// returns their `(offset, count)` in the array. `origin` is the owner's world position.
    pub fn push(
        &mut self,
        modifiers: Option<&Modifiers>,
        mouse: Option<&MouseAttractor>,
        origin: Vec2,
    ) -> (u32, u32) {
        let offset = self.modifiers.len() as u32;
        if let Some(modifiers) = modifiers {
            self.modifiers
                .extend(modifiers.iter().map(|m| GpuModifier::encode(m, origin)));
        }
        if let (Some(mouse), Some(target)) = (mouse, self.mouse_target) {
            let attractor = Modifier::PointAttractor {
                offset: target - origin,
                strength: mouse.strength,
                max_accel: mouse.max_accel,
            };
            self.modifiers.push(GpuModifier::encode(&attractor, origin));
        }
        (offset, self.modifiers.len() as u32 - offset)
    }

    /// The array to upload. Never empty, because a zero-sized storage buffer can't be bound.
    pub fn gpu_data(&self) -> Vec<GpuModifier> {
        if self.modifiers.is_empty() {
            vec![GpuModifier::default()]
        } else {
            self.modifiers.clone()
        }
    }
}

fn clear_modifier_upload(mut upload: ResMut<ModifierUpload>, mouse: Res<MouseTarget>) {
    upload.modifiers.clear();
    upload.mouse_target = mouse.0;
}
