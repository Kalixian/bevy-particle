use bevy::prelude::*;

/// Per-particle state and the system ordering every particle layer plugs into.
pub struct ParticlePlugin;

impl Plugin for ParticlePlugin {
    fn build(&self, app: &mut App) {
        app.configure_sets(
            Update,
            (
                ParticleSystems::Spawn,
                ParticleSystems::Forces,
                ParticleSystems::Integrate,
            )
                .chain(),
        )
        .add_systems(Startup, create_particle_mesh)
        .add_systems(Update, integrate.in_set(ParticleSystems::Integrate));
    }
}

/// Frame order: emitters spawn, then layers add forces to velocity, then
/// velocity moves position (semi-implicit Euler).
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub enum ParticleSystems {
    Spawn,
    Forces,
    Integrate,
}

#[derive(Component, Debug, Default)]
#[require(Velocity, Mass, Transform)]
pub struct Particle;

#[derive(Component, Debug, Default, Clone, Copy, Deref, DerefMut)]
pub struct Velocity(pub Vec2);

#[derive(Component, Debug, Clone, Copy, Deref, DerefMut)]
pub struct Mass(pub f32);

impl Default for Mass {
    fn default() -> Self {
        Self(1.0)
    }
}

/// The emitter that owns a particle. Despawning the emitter despawns its particles.
#[derive(Component, Debug)]
#[relationship(relationship_target = EmittedParticles)]
pub struct EmittedBy(pub Entity);

/// All particles owned by an emitter (maintained automatically by Bevy).
#[derive(Component, Debug, Default)]
#[relationship_target(relationship = EmittedBy, linked_spawn)]
pub struct EmittedParticles(Vec<Entity>);

/// Shared unit-circle mesh; particle size is applied through `Transform::scale`.
#[derive(Resource)]
pub struct ParticleMesh(pub Handle<Mesh>);

fn create_particle_mesh(mut commands: Commands, mut meshes: ResMut<Assets<Mesh>>) {
    commands.insert_resource(ParticleMesh(meshes.add(Circle::new(1.0))));
}

fn integrate(time: Res<Time>, mut particles: Query<(&mut Transform, &Velocity), With<Particle>>) {
    let dt = time.delta_secs();
    for (mut transform, velocity) in &mut particles {
        transform.translation += velocity.extend(0.0) * dt;
    }
}
